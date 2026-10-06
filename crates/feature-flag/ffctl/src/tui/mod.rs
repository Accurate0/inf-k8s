mod input;
mod view;

use crate::client::{Api, EvalInput};
use crate::commands::Commands;
use crate::convert::Convert;
use crate::editor::Editor;
use crate::plan::{Diff, DiffLine, DiffTag};
use crate::watch::SnapshotTracker;
use feature_flag_proto as pb;
use futures::StreamExt;
use input::Input;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Flags,
    Segments,
    Audit,
    Evaluate,
    Live,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Flags,
        Tab::Segments,
        Tab::Audit,
        Tab::Evaluate,
        Tab::Live,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Flags => "Flags",
            Tab::Segments => "Segments",
            Tab::Audit => "Audit",
            Tab::Evaluate => "Evaluate",
            Tab::Live => "Live",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    fn offset(self, by: isize) -> Self {
        let count = Self::ALL.len() as isize;
        let index = (self.index() as isize + by).rem_euclid(count);

        Self::ALL[index as usize]
    }
}

enum AppEvent {
    Key(KeyEvent),
    Redraw,
    Snapshot(pb::SnapshotResponse),
    StreamDown(String),
    Tick,
}

pub enum Pending {
    UpdateFlag {
        key: String,
        enabled: bool,
        default: String,
    },
    ArchiveFlag {
        key: String,
        archived: bool,
    },
    DeleteFlag {
        key: String,
    },
    ApplyFlag {
        current: Option<Box<pb::Flag>>,
        desired: Box<pb::Flag>,
    },
    ApplySegment {
        segment: pb::Segment,
    },
    DeleteSegment {
        key: String,
    },
}

pub struct Confirm {
    pub title: String,
    pub lines: Vec<DiffLine>,
    pub scroll: u16,
    pending: Pending,
}

pub struct Prompt {
    pub title: String,
    pub hint: String,
    pub input: Input,
    flag_key: String,
    enabled: bool,
}

pub enum Mode {
    Normal,
    Filter,
    Confirm(Confirm),
    Prompt(Prompt),
    Help,
    Eval,
}

enum EditTarget {
    Flag(Option<Box<pb::Flag>>),
    Segment(Option<pb::Segment>),
}

struct EditRequest {
    initial: String,
    name: String,
    target: EditTarget,
}

#[derive(Default)]
pub struct EvalForm {
    pub fields: [Input; 3],
    pub focus: usize,
    pub results: Vec<pb::EvaluatedFlag>,
    pub error: Option<String>,
}

impl EvalForm {
    pub const LABELS: [&'static str; 3] =
        ["Targeting key", "Flag (blank = all)", "Attributes JSON"];
}

#[derive(Default)]
pub struct Live {
    pub connected: bool,
    pub version: i64,
    pub log: VecDeque<(String, String)>,
    tracker: SnapshotTracker,
}

impl Live {
    const MAX_LOG: usize = 200;

    fn record(&mut self, message: String) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.log.push_front((time, message));
        self.log.truncate(Self::MAX_LOG);
    }
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

struct KeyReader {
    paused: Arc<AtomicBool>,
}

impl KeyReader {
    const POLL: Duration = Duration::from_millis(100);

    fn spawn(events: UnboundedSender<AppEvent>) -> Self {
        let paused = Arc::new(AtomicBool::new(false));
        let flag = paused.clone();

        std::thread::spawn(move || {
            loop {
                if flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Self::POLL);
                    continue;
                }

                if !event::poll(Self::POLL).unwrap_or(false) {
                    continue;
                }

                if flag.load(Ordering::Relaxed) {
                    continue;
                }

                let sent = match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        events.send(AppEvent::Key(key))
                    }
                    Ok(Event::Resize(_, _)) => events.send(AppEvent::Redraw),
                    Ok(_) => Ok(()),
                    Err(_) => return,
                };

                if sent.is_err() {
                    return;
                }
            }
        });

        Self { paused }
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        std::thread::sleep(Self::POLL + Duration::from_millis(50));
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }
}

pub struct App {
    api: Api,
    pub user: String,
    pub url: String,
    pub tab: Tab,
    pub mode: Mode,
    pub flags: Vec<pb::Flag>,
    pub flag_state: ListState,
    pub flag_filter: Input,
    pub show_archived: bool,
    pub segments: Vec<pb::Segment>,
    pub segment_state: ListState,
    pub changes: Vec<pb::FlagChange>,
    pub change_state: ListState,
    pub audit_filter: Option<(String, String)>,
    pub eval: EvalForm,
    pub live: Live,
    pub detail_scroll: u16,
    pub status: Option<Status>,
    quit: bool,
}

pub struct Tui;

impl Tui {
    pub async fn run(api: Api) -> anyhow::Result<()> {
        let mut app = App::new(api);
        app.reload().await;

        let mut terminal = ratatui::init();
        let result = app.event_loop(&mut terminal).await;
        ratatui::restore();

        result
    }
}

impl App {
    const AUDIT_LIMIT: u32 = 200;
    const STREAM_RETRY: Duration = Duration::from_secs(2);
    const TICK: Duration = Duration::from_secs(30);
    const PAGE: u16 = 10;

    fn new(api: Api) -> Self {
        let mut eval = EvalForm::default();
        eval.fields[2].set("{}");

        Self {
            user: api.user(),
            url: api.url().to_owned(),
            api,
            tab: Tab::Flags,
            mode: Mode::Normal,
            flags: Vec::new(),
            flag_state: ListState::default(),
            flag_filter: Input::default(),
            show_archived: false,
            segments: Vec::new(),
            segment_state: ListState::default(),
            changes: Vec::new(),
            change_state: ListState::default(),
            audit_filter: None,
            eval,
            live: Live::default(),
            detail_scroll: 0,
            status: None,
            quit: false,
        }
    }

    async fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let (sender, mut events) = unbounded_channel();
        let keys = KeyReader::spawn(sender.clone());

        let stream = tokio::spawn(Self::follow_snapshots(self.api.clone(), sender.clone()));
        let ticker = tokio::spawn(Self::tick(sender));

        let result = self.pump(terminal, &mut events, &keys).await;

        stream.abort();
        ticker.abort();
        keys.pause();

        result
    }

    async fn pump(
        &mut self,
        terminal: &mut DefaultTerminal,
        events: &mut UnboundedReceiver<AppEvent>,
        keys: &KeyReader,
    ) -> anyhow::Result<()> {
        while !self.quit {
            terminal.draw(|frame| self.draw(frame))?;

            let Some(event) = events.recv().await else {
                break;
            };

            match event {
                AppEvent::Key(key) => {
                    let Some(request) = self.on_key(key).await else {
                        continue;
                    };

                    keys.pause();
                    ratatui::restore();
                    let edited = Editor::edit(&request.initial, &request.name);
                    *terminal = ratatui::init();
                    keys.resume();

                    self.after_edit(request, edited);
                }
                AppEvent::Redraw => {}
                AppEvent::Snapshot(snapshot) => self.on_snapshot(snapshot).await,
                AppEvent::StreamDown(reason) => {
                    self.live.connected = false;
                    self.live.record(format!("disconnected: {reason}"));
                }
                AppEvent::Tick => {
                    if let Err(e) = self.api.keep_fresh().await {
                        self.fail(e);
                    }
                }
            }
        }

        Ok(())
    }

    async fn follow_snapshots(api: Api, events: UnboundedSender<AppEvent>) {
        loop {
            let reason = match api.stream_snapshot().await {
                Ok(mut stream) => loop {
                    match stream.next().await {
                        Some(Ok(snapshot)) => {
                            if events.send(AppEvent::Snapshot(snapshot)).is_err() {
                                return;
                            }
                        }
                        Some(Err(status)) => break status.message().to_owned(),
                        None => break "stream closed".to_owned(),
                    }
                },
                Err(e) => e.to_string(),
            };

            if events.send(AppEvent::StreamDown(reason)).is_err() {
                return;
            }

            tokio::time::sleep(Self::STREAM_RETRY).await;
        }
    }

    async fn tick(events: UnboundedSender<AppEvent>) {
        let mut interval = tokio::time::interval(Self::TICK);

        loop {
            interval.tick().await;

            if events.send(AppEvent::Tick).is_err() {
                return;
            }
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
    }

    fn fail(&mut self, error: impl std::fmt::Display) {
        self.status = Some(Status {
            text: error.to_string(),
            error: true,
        });
    }

    pub fn visible_flags(&self) -> Vec<&pb::Flag> {
        let filter = self.flag_filter.value().to_lowercase();

        self.flags
            .iter()
            .filter(|flag| filter.is_empty() || flag.key.to_lowercase().contains(&filter))
            .collect()
    }

    pub fn selected_flag(&self) -> Option<&pb::Flag> {
        let visible = self.visible_flags();

        self.flag_state
            .selected()
            .and_then(|index| visible.get(index).copied())
    }

    pub fn selected_segment(&self) -> Option<&pb::Segment> {
        self.segment_state
            .selected()
            .and_then(|index| self.segments.get(index))
    }

    pub fn selected_change(&self) -> Option<&pb::FlagChange> {
        self.change_state
            .selected()
            .and_then(|index| self.changes.get(index))
    }

    fn clamp(state: &mut ListState, len: usize) {
        if len == 0 {
            state.select(None);
            return;
        }

        state.select(Some(state.selected().unwrap_or(0).min(len - 1)));
    }

    fn clamp_all(&mut self) {
        let flags = self.visible_flags().len();

        Self::clamp(&mut self.flag_state, flags);
        Self::clamp(&mut self.segment_state, self.segments.len());
        Self::clamp(&mut self.change_state, self.changes.len());
    }

    async fn reload(&mut self) {
        match self.api.list_flags(self.show_archived).await {
            Ok(flags) => self.flags = flags,
            Err(e) => self.fail(e),
        }

        match self.api.list_segments().await {
            Ok(segments) => self.segments = segments,
            Err(e) => self.fail(e),
        }

        self.reload_changes().await;
        self.clamp_all();
    }

    async fn reload_changes(&mut self) {
        let (kind, key) = self.audit_filter.clone().unwrap_or_default();

        match self.api.list_changes(&kind, &key, Self::AUDIT_LIMIT).await {
            Ok(changes) => self.changes = changes,
            Err(e) => self.fail(e),
        }

        Self::clamp(&mut self.change_state, self.changes.len());
    }

    async fn on_snapshot(&mut self, snapshot: pb::SnapshotResponse) {
        let delta = self.live.tracker.observe(snapshot);

        self.live.connected = true;
        self.live.version = delta.version;
        self.live.record(delta.summary());

        if !delta.initial {
            self.reload().await;
        }
    }

    fn move_selection(&mut self, by: isize) {
        let (state, len) = match self.tab {
            Tab::Flags => {
                let len = self.visible_flags().len();
                (&mut self.flag_state, len)
            }
            Tab::Segments => (&mut self.segment_state, self.segments.len()),
            Tab::Audit => (&mut self.change_state, self.changes.len()),
            Tab::Evaluate | Tab::Live => return,
        };

        if len == 0 {
            return;
        }

        let current = state.selected().unwrap_or(0) as isize;
        let next = (current + by).clamp(0, len as isize - 1);

        state.select(Some(next as usize));
        self.detail_scroll = 0;
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.detail_scroll = 0;
    }

    async fn on_key(&mut self, key: KeyEvent) -> Option<EditRequest> {
        let interrupt =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');

        if interrupt {
            self.quit = true;
            return None;
        }

        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => return self.on_normal_key(key).await,
            Mode::Help => {}
            Mode::Filter => self.on_filter_key(key),
            Mode::Confirm(confirm) => self.on_confirm_key(key, confirm).await,
            Mode::Prompt(prompt) => self.on_prompt_key(key, prompt),
            Mode::Eval => self.on_eval_key(key).await,
        }

        None
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {}
            KeyCode::Esc => self.flag_filter.clear(),
            _ => {
                self.flag_filter.handle(key);
                self.mode = Mode::Filter;
            }
        }

        self.flag_state.select(Some(0));
        self.clamp_all();
    }

    async fn on_confirm_key(&mut self, key: KeyEvent, mut confirm: Confirm) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => self.execute(confirm.pending).await,
            KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => self.info("cancelled"),
            KeyCode::Down | KeyCode::Char('j') => {
                confirm.scroll = confirm.scroll.saturating_add(1);
                self.mode = Mode::Confirm(confirm);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                confirm.scroll = confirm.scroll.saturating_sub(1);
                self.mode = Mode::Confirm(confirm);
            }
            _ => self.mode = Mode::Confirm(confirm),
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent, mut prompt: Prompt) {
        match key.code {
            KeyCode::Esc => self.info("cancelled"),
            KeyCode::Enter => {
                let default = prompt.input.value().trim().to_owned();

                self.mode = Mode::Confirm(Confirm {
                    title: format!("Set default of {} to `{default}`?", prompt.flag_key),
                    lines: Vec::new(),
                    scroll: 0,
                    pending: Pending::UpdateFlag {
                        key: prompt.flag_key,
                        enabled: prompt.enabled,
                        default,
                    },
                });
            }
            _ => {
                prompt.input.handle(key);
                self.mode = Mode::Prompt(prompt);
            }
        }
    }

    async fn on_eval_key(&mut self, key: KeyEvent) {
        let fields = self.eval.fields.len();

        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => self.evaluate().await,
            KeyCode::Tab | KeyCode::Down => self.eval.focus = (self.eval.focus + 1) % fields,
            KeyCode::BackTab | KeyCode::Up => {
                self.eval.focus = (self.eval.focus + fields - 1) % fields
            }
            _ => {
                self.eval.fields[self.eval.focus].handle(key);
            }
        }

        self.mode = Mode::Eval;
    }

    async fn evaluate(&mut self) {
        let targeting_key = self.eval.fields[0].value().trim().to_owned();
        let flag_key = self.eval.fields[1].value().trim().to_owned();

        let attributes = match Commands::eval_attributes(&[], Some(self.eval.fields[2].value())) {
            Ok(attributes) => attributes,
            Err(e) => {
                self.eval.error = Some(e.to_string());
                return;
            }
        };

        let input = EvalInput {
            targeting_key,
            attributes,
        };

        let evaluated = if flag_key.is_empty() {
            self.api.resolve_all(&input).await
        } else {
            self.api
                .resolve(&flag_key, &input)
                .await
                .map(|flag| vec![flag])
        };

        match evaluated {
            Ok(results) => {
                self.eval.results = results;
                self.eval.error = None;
            }
            Err(e) => self.eval.error = Some(e.to_string()),
        }
    }

    async fn on_normal_key(&mut self, key: KeyEvent) -> Option<EditRequest> {
        self.status = None;

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('r') => {
                self.reload().await;

                if self.status.is_none() {
                    self.info("reloaded");
                }
            }
            KeyCode::Char(digit @ '1'..='5') => {
                let index = digit as usize - '1' as usize;
                self.switch_tab(Tab::ALL[index]);
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.switch_tab(self.tab.offset(1))
            }
            KeyCode::BackTab | KeyCode::Left => self.switch_tab(self.tab.offset(-1)),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX / 2),
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(Self::PAGE),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(Self::PAGE),
            _ => {
                return match self.tab {
                    Tab::Flags => self.on_flags_key(key).await,
                    Tab::Segments => self.on_segments_key(key).await,
                    Tab::Audit => {
                        self.on_audit_key(key).await;
                        None
                    }
                    Tab::Evaluate => {
                        if matches!(key.code, KeyCode::Enter | KeyCode::Char('i' | 'e')) {
                            self.mode = Mode::Eval;
                        }
                        None
                    }
                    Tab::Live => None,
                };
            }
        }

        None
    }

    async fn on_flags_key(&mut self, key: KeyEvent) -> Option<EditRequest> {
        match key.code {
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                return None;
            }
            KeyCode::Esc => {
                self.flag_filter.clear();
                self.clamp_all();
                return None;
            }
            KeyCode::Char('a') => {
                self.show_archived = !self.show_archived;
                self.reload().await;
                return None;
            }
            KeyCode::Char('n') => {
                return Some(EditRequest {
                    initial: Editor::FLAG_TEMPLATE.to_owned(),
                    name: "new-flag".to_owned(),
                    target: EditTarget::Flag(None),
                });
            }
            _ => {}
        }

        let flag = self.selected_flag()?.clone();

        match key.code {
            KeyCode::Char(' ') | KeyCode::Char('t') => {
                let enabled = !flag.enabled;
                let verb = if enabled { "Enable" } else { "Disable" };

                self.mode = Mode::Confirm(Confirm {
                    title: format!("{verb} {}?", flag.key),
                    lines: Vec::new(),
                    scroll: 0,
                    pending: Pending::UpdateFlag {
                        key: flag.key,
                        enabled,
                        default: flag.default_variant_key,
                    },
                });
            }
            KeyCode::Char('d') => {
                let variants: Vec<&str> = flag
                    .variants
                    .iter()
                    .map(|variant| variant.key.as_str())
                    .collect();

                self.mode = Mode::Prompt(Prompt {
                    title: format!("Default variant for {}", flag.key),
                    hint: format!("variants: {}", variants.join(", ")),
                    input: Input::with_value(&flag.default_variant_key),
                    flag_key: flag.key.clone(),
                    enabled: flag.enabled,
                });
            }
            KeyCode::Char('x') => {
                let archived = !flag.archived;
                let verb = if archived { "Archive" } else { "Restore" };

                self.mode = Mode::Confirm(Confirm {
                    title: format!("{verb} {}?", flag.key),
                    lines: Vec::new(),
                    scroll: 0,
                    pending: Pending::ArchiveFlag {
                        key: flag.key,
                        archived,
                    },
                });
            }
            KeyCode::Char('D') => {
                self.mode = Mode::Confirm(Confirm {
                    title: format!("Permanently delete flag {}?", flag.key),
                    lines: Self::removal(&Convert::flag_yaml(&flag)),
                    scroll: 0,
                    pending: Pending::DeleteFlag { key: flag.key },
                });
            }
            KeyCode::Char('h') => {
                self.audit_filter = Some(("flag".to_owned(), flag.key));
                self.switch_tab(Tab::Audit);
                self.reload_changes().await;
            }
            KeyCode::Char('v') => {
                self.eval.fields[1].set(&flag.key);
                self.eval.focus = 0;
                self.switch_tab(Tab::Evaluate);
                self.mode = Mode::Eval;
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                return Some(EditRequest {
                    initial: Convert::flag_yaml(&flag),
                    name: flag.key.clone(),
                    target: EditTarget::Flag(Some(Box::new(flag))),
                });
            }
            _ => {}
        }

        None
    }

    async fn on_segments_key(&mut self, key: KeyEvent) -> Option<EditRequest> {
        if key.code == KeyCode::Char('n') {
            return Some(EditRequest {
                initial: Editor::SEGMENT_TEMPLATE.to_owned(),
                name: "new-segment".to_owned(),
                target: EditTarget::Segment(None),
            });
        }

        let segment = self.selected_segment()?.clone();

        match key.code {
            KeyCode::Char('D') => {
                self.mode = Mode::Confirm(Confirm {
                    title: format!("Delete segment {}?", segment.key),
                    lines: Self::removal(&Convert::segment_yaml(&segment)),
                    scroll: 0,
                    pending: Pending::DeleteSegment { key: segment.key },
                });
            }
            KeyCode::Char('h') => {
                self.audit_filter = Some(("segment".to_owned(), segment.key));
                self.switch_tab(Tab::Audit);
                self.reload_changes().await;
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                return Some(EditRequest {
                    initial: Convert::segment_yaml(&segment),
                    name: segment.key.clone(),
                    target: EditTarget::Segment(Some(segment)),
                });
            }
            _ => {}
        }

        None
    }

    async fn on_audit_key(&mut self, key: KeyEvent) {
        if !matches!(key.code, KeyCode::Char('c') | KeyCode::Esc) {
            return;
        }

        self.audit_filter = None;
        self.reload_changes().await;
    }

    fn removal(yaml: &str) -> Vec<DiffLine> {
        yaml.lines()
            .map(|line| DiffLine {
                tag: DiffTag::Delete,
                text: line.to_owned(),
            })
            .collect()
    }

    fn after_edit(&mut self, request: EditRequest, edited: anyhow::Result<String>) {
        let edited = match edited {
            Ok(edited) => edited,
            Err(e) => return self.fail(e),
        };

        let outcome = match request.target {
            EditTarget::Flag(current) => self.review_flag(current, &edited),
            EditTarget::Segment(current) => self.review_segment(current, &edited),
        };

        match outcome {
            Ok(Some(confirm)) => self.mode = Mode::Confirm(confirm),
            Ok(None) => self.info("no changes"),
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    fn review_flag(
        &self,
        current: Option<Box<pb::Flag>>,
        edited: &str,
    ) -> anyhow::Result<Option<Confirm>> {
        let desired = Convert::flag_from_yaml(edited)?;
        let after = Convert::flag_yaml(&desired);

        let before = match &current {
            Some(current) => Convert::flag_yaml(current),
            None if self.flags.iter().any(|flag| flag.key == desired.key) => {
                anyhow::bail!("flag {} already exists", desired.key)
            }
            None => String::new(),
        };

        if before == after {
            return Ok(None);
        }

        let verb = if current.is_some() {
            "Update"
        } else {
            "Create"
        };

        Ok(Some(Confirm {
            title: format!("{verb} flag {}?", desired.key),
            lines: Diff::lines(&before, &after),
            scroll: 0,
            pending: Pending::ApplyFlag {
                current,
                desired: Box::new(desired),
            },
        }))
    }

    fn review_segment(
        &self,
        current: Option<pb::Segment>,
        edited: &str,
    ) -> anyhow::Result<Option<Confirm>> {
        let desired = Convert::segment_from_yaml(edited)?;
        let after = Convert::segment_yaml(&desired);

        let before = match &current {
            Some(current) if current.key != desired.key => {
                anyhow::bail!("a segment's key cannot be changed; create a new segment instead")
            }
            Some(current) => Convert::segment_yaml(current),
            None if self
                .segments
                .iter()
                .any(|segment| segment.key == desired.key) =>
            {
                anyhow::bail!("segment {} already exists", desired.key)
            }
            None => String::new(),
        };

        if before == after {
            return Ok(None);
        }

        let verb = if current.is_some() {
            "Update"
        } else {
            "Create"
        };

        Ok(Some(Confirm {
            title: format!("{verb} segment {}?", desired.key),
            lines: Diff::lines(&before, &after),
            scroll: 0,
            pending: Pending::ApplySegment { segment: desired },
        }))
    }

    async fn execute(&mut self, pending: Pending) {
        let outcome = match pending {
            Pending::UpdateFlag {
                key,
                enabled,
                default,
            } => self
                .api
                .update_flag(&key, enabled, &default)
                .await
                .map(|_| format!("updated {key}")),
            Pending::ArchiveFlag { key, archived } => {
                let verb = if archived { "archived" } else { "restored" };

                self.api
                    .archive_flag(&key, archived)
                    .await
                    .map(|_| format!("{verb} {key}"))
            }
            Pending::DeleteFlag { key } => self
                .api
                .delete_flag(&key)
                .await
                .map(|_| format!("deleted {key}")),
            Pending::ApplyFlag { current, desired } => self
                .api
                .apply_flag(current.as_deref(), &desired)
                .await
                .map(|flag| format!("saved {}", flag.key)),
            Pending::ApplySegment { segment } => self
                .api
                .upsert_segment(segment)
                .await
                .map(|segment| format!("saved {}", segment.key)),
            Pending::DeleteSegment { key } => self
                .api
                .delete_segment(&key)
                .await
                .map(|_| format!("deleted {key}")),
        };

        self.reload().await;

        match outcome {
            Ok(message) => self.info(message),
            Err(e) => self.fail(e),
        }
    }
}
