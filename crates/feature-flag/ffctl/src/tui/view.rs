use super::input::Input;
use super::{App, Confirm, EvalForm, Mode, Prompt, Tab};
use crate::convert::Convert;
use crate::plan::DiffTag;
use feature_flag_proto as pb;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table, Tabs, Wrap};

struct Theme;

impl Theme {
    const ACCENT: Color = Color::Cyan;
    const MUTED: Color = Color::DarkGray;
    const GOOD: Color = Color::Green;
    const BAD: Color = Color::Red;
    const WARN: Color = Color::Yellow;

    fn selected() -> Style {
        Style::new()
            .bg(Color::Indexed(237))
            .add_modifier(Modifier::BOLD)
    }

    fn block(title: String) -> Block<'static> {
        Block::bordered()
            .border_style(Style::new().fg(Self::MUTED))
            .title(Span::styled(title, Style::new().fg(Self::ACCENT).bold()))
    }
}

impl App {
    pub fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_header(frame, header);

        match self.tab {
            Tab::Flags => self.draw_flags(frame, body),
            Tab::Segments => self.draw_segments(frame, body),
            Tab::Audit => self.draw_audit(frame, body),
            Tab::Evaluate => self.draw_evaluate(frame, body),
            Tab::Live => self.draw_live(frame, body),
        }

        self.draw_footer(frame, footer);

        match &self.mode {
            Mode::Confirm(confirm) => Self::draw_confirm(frame, confirm),
            Mode::Prompt(prompt) => Self::draw_prompt(frame, prompt),
            Mode::Help => Self::draw_help(frame),
            Mode::Normal | Mode::Filter | Mode::Eval => {}
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<String> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(index, tab)| format!("{} {}", index + 1, tab.title()))
            .collect();

        let (dot, state) = if self.live.connected {
            (Span::styled("●", Style::new().fg(Theme::GOOD)), "live")
        } else {
            (Span::styled("●", Style::new().fg(Theme::BAD)), "offline")
        };

        let session = Line::from(vec![
            Span::raw(" "),
            dot,
            Span::raw(format!(" {state} v{}  ", self.live.version)),
            Span::styled(self.user.clone(), Style::new().bold()),
            Span::styled(format!(" @ {} ", self.url), Style::new().fg(Theme::MUTED)),
        ])
        .right_aligned();

        let tabs = Tabs::new(titles)
            .select(self.tab.index())
            .highlight_style(Style::new().fg(Theme::ACCENT).bold().underlined())
            .divider("│")
            .block(Theme::block(" ffctl ".to_owned()).title_top(session));

        frame.render_widget(tabs, area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if let Some(status) = &self.status {
            let color = if status.error {
                Theme::BAD
            } else {
                Theme::GOOD
            };
            let line = Line::from(Span::styled(
                format!(" {}", status.text),
                Style::new().fg(color),
            ));

            frame.render_widget(Paragraph::new(line), area);
            return;
        }

        let hints = match (&self.mode, self.tab) {
            (Mode::Filter, _) => "type to filter  enter keep  esc clear",
            (Mode::Eval, _) => "tab next field  enter evaluate  esc done",
            (_, Tab::Flags) => {
                "/ filter  space toggle  d default  e edit  n new  x archive  D delete  h history  v evaluate  a archived  ? help"
            }
            (_, Tab::Segments) => "e edit  n new  D delete  h history  r reload  ? help",
            (_, Tab::Audit) => "c clear filter  r reload  ? help",
            (_, Tab::Evaluate) => "enter edit and evaluate  ? help",
            (_, Tab::Live) => "r reload  ? help  q quit",
        };

        let line = Line::from(Span::styled(
            format!(" {hints}"),
            Style::new().fg(Theme::MUTED),
        ));

        frame.render_widget(Paragraph::new(line), area);
    }

    fn split(area: Rect, left: u16) -> [Rect; 2] {
        Layout::horizontal([
            Constraint::Percentage(left),
            Constraint::Percentage(100 - left),
        ])
        .areas(area)
    }

    fn yaml_lines(yaml: &str) -> Vec<Line<'static>> {
        yaml.lines()
            .map(|line| {
                let indent = line.len() - line.trim_start().len();
                let rest = &line[indent..];
                let (marker, rest) = match rest.strip_prefix("- ") {
                    Some(stripped) => ("- ", stripped),
                    None => ("", rest),
                };

                let Some((key, value)) = rest.split_once(':') else {
                    return Line::raw(line.to_owned());
                };

                let is_key = !key.is_empty() && !key.contains(' ') && !key.starts_with('"')
                    || key.starts_with('"') && key.ends_with('"');

                if !is_key {
                    return Line::raw(line.to_owned());
                }

                Line::from(vec![
                    Span::raw(format!("{}{marker}", " ".repeat(indent))),
                    Span::styled(key.to_owned(), Style::new().fg(Theme::ACCENT)),
                    Span::raw(format!(":{value}")),
                ])
            })
            .collect()
    }

    fn detail(&self, frame: &mut Frame, area: Rect, title: String, lines: Vec<Line<'static>>) {
        let paragraph = Paragraph::new(lines)
            .block(Theme::block(title))
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0));

        frame.render_widget(paragraph, area);
    }

    fn draw_flags(&mut self, frame: &mut Frame, area: Rect) {
        let [left, right] = Self::split(area, 40);

        let items: Vec<ListItem> = self
            .visible_flags()
            .into_iter()
            .map(|flag| {
                let dot = if flag.enabled {
                    Span::styled("● ", Style::new().fg(Theme::GOOD))
                } else {
                    Span::styled("○ ", Style::new().fg(Theme::BAD))
                };

                let key_style = if flag.archived {
                    Style::new().fg(Theme::MUTED).crossed_out()
                } else {
                    Style::new()
                };

                ListItem::new(Line::from(vec![
                    dot,
                    Span::styled(flag.key.clone(), key_style),
                    Span::styled(
                        format!(
                            "  {} → {}",
                            Convert::value_type_name(flag.value_type),
                            flag.default_variant_key
                        ),
                        Style::new().fg(Theme::MUTED),
                    ),
                ]))
            })
            .collect();

        let filtering = matches!(self.mode, Mode::Filter);
        let filter = self.flag_filter.value();

        let mut title = format!(" Flags ({}) ", items.len());
        if filtering || !filter.is_empty() {
            title = format!(" Flags ({}) /{filter} ", items.len());
        }
        if self.show_archived {
            title.push_str("+archived ");
        }

        if filtering {
            let prefix = format!(" Flags ({}) /", items.len()).chars().count() as u16;
            let x = left.x + 1 + prefix + self.flag_filter.cursor() as u16;
            frame.set_cursor_position(Position::new(x.min(left.right().saturating_sub(2)), left.y));
        }

        let list = List::new(items)
            .block(Theme::block(title))
            .highlight_style(Theme::selected())
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, left, &mut self.flag_state);

        let Some(flag) = self.selected_flag() else {
            self.detail(
                frame,
                right,
                " Flag ".to_owned(),
                vec![Line::styled(
                    "no flags; press n to create one",
                    Style::new().fg(Theme::MUTED),
                )],
            );
            return;
        };

        let state = if flag.enabled {
            Span::styled("enabled", Style::new().fg(Theme::GOOD).bold())
        } else {
            Span::styled("disabled", Style::new().fg(Theme::BAD).bold())
        };

        let mut summary = vec![
            state,
            Span::styled(
                format!(
                    "  {} variant(s), {} rule(s)",
                    flag.variants.len(),
                    flag.rules.len()
                ),
                Style::new().fg(Theme::MUTED),
            ),
        ];

        if flag.archived {
            summary.push(Span::styled(
                "  archived",
                Style::new().fg(Theme::WARN).bold(),
            ));
        }

        let mut lines = vec![Line::from(summary), Line::raw("")];
        lines.extend(Self::yaml_lines(&Convert::flag_yaml(flag)));

        self.detail(frame, right, format!(" {} ", flag.key), lines);
    }

    fn draw_segments(&mut self, frame: &mut Frame, area: Rect) {
        let [left, right] = Self::split(area, 40);

        let items: Vec<ListItem> = self
            .segments
            .iter()
            .map(|segment| {
                ListItem::new(Line::from(vec![
                    Span::raw(segment.key.clone()),
                    Span::styled(format!("  {}", segment.name), Style::new().fg(Theme::MUTED)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(Theme::block(format!(
                " Segments ({}) ",
                self.segments.len()
            )))
            .highlight_style(Theme::selected())
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, left, &mut self.segment_state);

        let Some(segment) = self.selected_segment() else {
            self.detail(
                frame,
                right,
                " Segment ".to_owned(),
                vec![Line::styled(
                    "no segments; press n to create one",
                    Style::new().fg(Theme::MUTED),
                )],
            );
            return;
        };

        let used_by: Vec<&str> = self
            .flags
            .iter()
            .filter(|flag| {
                flag.rules
                    .iter()
                    .any(|rule| rule.segment_key == segment.key)
            })
            .map(|flag| flag.key.as_str())
            .collect();

        let usage = if used_by.is_empty() {
            "not referenced by any flag".to_owned()
        } else {
            format!("used by: {}", used_by.join(", "))
        };

        let mut lines = vec![
            Line::styled(usage, Style::new().fg(Theme::MUTED)),
            Line::raw(""),
        ];
        lines.extend(Self::yaml_lines(&Convert::segment_yaml(segment)));

        self.detail(frame, right, format!(" {} ", segment.key), lines);
    }

    fn draw_audit(&mut self, frame: &mut Frame, area: Rect) {
        let [left, right] = Self::split(area, 55);

        let items: Vec<ListItem> = self
            .changes
            .iter()
            .map(|change| {
                let when = change.created_at.get(..19).unwrap_or(&change.created_at);

                ListItem::new(Line::from(vec![
                    Span::styled(format!("{when}  "), Style::new().fg(Theme::MUTED)),
                    Span::styled(
                        format!("{}  ", change.actor),
                        Style::new().fg(Theme::ACCENT),
                    ),
                    Span::raw(format!("{}  ", change.action)),
                    Span::styled(
                        format!("{}/{}", change.target_kind, change.target_key),
                        Style::new().fg(Theme::MUTED),
                    ),
                ]))
            })
            .collect();

        let title = match &self.audit_filter {
            Some((kind, key)) => format!(" Audit ({}) {kind}/{key} ", self.changes.len()),
            None => format!(" Audit ({}) ", self.changes.len()),
        };

        let list = List::new(items)
            .block(Theme::block(title))
            .highlight_style(Theme::selected())
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, left, &mut self.change_state);

        let Some(change) = self.selected_change() else {
            self.detail(
                frame,
                right,
                " Change ".to_owned(),
                vec![Line::styled(
                    "no changes recorded",
                    Style::new().fg(Theme::MUTED),
                )],
            );
            return;
        };

        let field = |name: &str, value: String| {
            Line::from(vec![
                Span::styled(format!("{name:<9}"), Style::new().fg(Theme::ACCENT)),
                Span::raw(value),
            ])
        };

        let detail = serde_json::to_string_pretty(&Convert::parse_json(&change.detail))
            .unwrap_or_else(|_| change.detail.clone());

        let mut lines = vec![
            field("when", change.created_at.clone()),
            field("actor", change.actor.clone()),
            field("action", change.action.clone()),
            field(
                "target",
                format!("{}/{}", change.target_kind, change.target_key),
            ),
            field("version", change.version.to_string()),
            Line::raw(""),
        ];
        lines.extend(detail.lines().map(|line| Line::raw(line.to_owned())));

        self.detail(frame, right, " Change ".to_owned(), lines);
    }

    fn input_line(label: &str, input: &Input, focused: bool) -> Line<'static> {
        let label_style = if focused {
            Style::new().fg(Theme::ACCENT).bold()
        } else {
            Style::new().fg(Theme::MUTED)
        };

        Line::from(vec![
            Span::styled(format!("{label:<20}"), label_style),
            Span::raw(input.value().to_owned()),
        ])
    }

    fn draw_evaluate(&mut self, frame: &mut Frame, area: Rect) {
        let [form, results] =
            Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(area);

        let editing = matches!(self.mode, Mode::Eval);

        let lines: Vec<Line> = EvalForm::LABELS
            .iter()
            .zip(&self.eval.fields)
            .enumerate()
            .map(|(index, (label, input))| {
                Self::input_line(label, input, editing && index == self.eval.focus)
            })
            .collect();

        frame.render_widget(
            Paragraph::new(lines).block(Theme::block(" Context ".to_owned())),
            form,
        );

        if editing {
            let input = &self.eval.fields[self.eval.focus];
            let x = form.x + 1 + 20 + input.cursor() as u16;
            let y = form.y + 1 + self.eval.focus as u16;

            frame.set_cursor_position(Position::new(x.min(form.right().saturating_sub(2)), y));
        }

        if let Some(error) = &self.eval.error {
            let paragraph =
                Paragraph::new(Line::styled(error.clone(), Style::new().fg(Theme::BAD)))
                    .wrap(Wrap { trim: false })
                    .block(Theme::block(" Error ".to_owned()));

            frame.render_widget(paragraph, results);
            return;
        }

        let rows: Vec<Row> = self
            .eval
            .results
            .iter()
            .map(|flag| {
                let meta = flag.meta.clone().unwrap_or_default();
                let reason = Convert::reason_name(meta.reason);

                let reason_color = match pb::Reason::try_from(meta.reason).unwrap_or_default() {
                    pb::Reason::TargetingMatch | pb::Reason::Split => Theme::GOOD,
                    pb::Reason::Error => Theme::BAD,
                    pb::Reason::Disabled => Theme::WARN,
                    _ => Theme::MUTED,
                };

                let value = flag
                    .value
                    .as_ref()
                    .map(Convert::value_to_json)
                    .map(|value| value.to_string())
                    .unwrap_or_default();

                Row::new(vec![
                    Cell::from(flag.flag_key.clone()),
                    Cell::from(value),
                    Cell::from(meta.variant),
                    Cell::from(Span::styled(reason, Style::new().fg(reason_color))),
                    Cell::from(Span::styled(meta.error_code, Style::new().fg(Theme::BAD))),
                ])
            })
            .collect();

        let widths = [
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
        ];

        let table = Table::new(rows, widths)
            .header(
                Row::new(vec!["FLAG", "VALUE", "VARIANT", "REASON", "ERROR"])
                    .style(Style::new().fg(Theme::ACCENT).bold()),
            )
            .block(Theme::block(format!(
                " Results ({}) ",
                self.eval.results.len()
            )));

        frame.render_widget(table, results);
    }

    fn draw_live(&mut self, frame: &mut Frame, area: Rect) {
        let [summary, log] =
            Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(area);

        let state = if self.live.connected {
            Span::styled("connected", Style::new().fg(Theme::GOOD).bold())
        } else {
            Span::styled("disconnected, retrying", Style::new().fg(Theme::BAD).bold())
        };

        let lines = vec![
            Line::from(vec![Span::raw("stream          "), state]),
            Line::raw(format!("config version  {}", self.live.version)),
            Line::raw(format!(
                "loaded          {} flag(s), {} segment(s)",
                self.flags.len(),
                self.segments.len()
            )),
        ];

        frame.render_widget(
            Paragraph::new(lines).block(Theme::block(" Snapshot stream ".to_owned())),
            summary,
        );

        let items: Vec<ListItem> = self
            .live
            .log
            .iter()
            .map(|(time, message)| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{time}  "), Style::new().fg(Theme::MUTED)),
                    Span::raw(message.clone()),
                ]))
            })
            .collect();

        frame.render_widget(
            List::new(items).block(Theme::block(" Events ".to_owned())),
            log,
        );
    }

    fn popup(frame: &Frame, width: u16, height: u16) -> Rect {
        let area = frame.area();
        let width = width.min(area.width.saturating_sub(4));
        let height = height.min(area.height.saturating_sub(2));

        Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        }
    }

    fn draw_confirm(frame: &mut Frame, confirm: &Confirm) {
        let height = (confirm.lines.len() as u16 + 4).max(5);
        let area = Self::popup(frame, 90, height);

        let mut lines: Vec<Line> = confirm
            .lines
            .iter()
            .map(|line| match line.tag {
                DiffTag::Insert => {
                    Line::styled(format!("+ {}", line.text), Style::new().fg(Theme::GOOD))
                }
                DiffTag::Delete => {
                    Line::styled(format!("- {}", line.text), Style::new().fg(Theme::BAD))
                }
                DiffTag::Equal => {
                    Line::styled(format!("  {}", line.text), Style::new().fg(Theme::MUTED))
                }
            })
            .collect();

        if lines.is_empty() {
            lines.push(Line::raw(""));
        }

        let [content, actions] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
            .areas(Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(2),
            });

        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::bordered()
                .border_style(Style::new().fg(Theme::WARN))
                .title(Span::styled(
                    format!(" {} ", confirm.title),
                    Style::new().fg(Theme::WARN).bold(),
                )),
            area,
        );
        frame.render_widget(Paragraph::new(lines).scroll((confirm.scroll, 0)), content);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("y", Style::new().fg(Theme::GOOD).bold()),
                Span::raw(" confirm   "),
                Span::styled("n", Style::new().fg(Theme::BAD).bold()),
                Span::raw(" cancel   "),
                Span::styled("j/k scroll", Style::new().fg(Theme::MUTED)),
            ])),
            actions,
        );
    }

    fn draw_prompt(frame: &mut Frame, prompt: &Prompt) {
        let area = Self::popup(frame, 70, 5);

        let lines = vec![
            Line::raw(prompt.input.value().to_owned()),
            Line::raw(""),
            Line::styled(prompt.hint.clone(), Style::new().fg(Theme::MUTED)),
        ];

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(Theme::block(format!(" {} ", prompt.title))),
            area,
        );

        let x = area.x + 1 + prompt.input.cursor() as u16;
        frame.set_cursor_position(Position::new(
            x.min(area.right().saturating_sub(2)),
            area.y + 1,
        ));
    }

    fn draw_help(frame: &mut Frame) {
        const BINDINGS: [(&str, &str); 19] = [
            ("1-5, tab, ←/→", "switch tab"),
            ("j/k, ↑/↓, g/G", "move selection"),
            ("PgUp/PgDn", "scroll the detail pane"),
            ("r", "reload from the server"),
            ("q, ctrl-c", "quit"),
            ("", ""),
            ("/", "flags: filter by key"),
            ("space, t", "flags: toggle enabled"),
            ("d", "flags: change the default variant"),
            ("e, enter", "flags/segments: edit as YAML in $EDITOR"),
            ("n", "flags/segments: create from a template"),
            ("x", "flags: archive or restore"),
            ("D", "flags/segments: delete"),
            ("h", "flags/segments: show history in Audit"),
            ("v", "flags: evaluate this flag"),
            ("a", "flags: show archived flags"),
            ("c", "audit: clear the filter"),
            ("enter", "evaluate: edit the context, enter again to run"),
            ("", "every write shows a diff and asks before applying"),
        ];

        let area = Self::popup(frame, 72, BINDINGS.len() as u16 + 2);

        let lines: Vec<Line> = BINDINGS
            .iter()
            .map(|(keys, action)| {
                Line::from(vec![
                    Span::styled(
                        format!(" {keys:<16}"),
                        Style::new().fg(Theme::ACCENT).bold(),
                    ),
                    Span::raw(*action),
                ])
            })
            .collect();

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(Theme::block(" Keys ".to_owned())),
            area,
        );
    }
}
