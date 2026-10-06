use anyhow::{Context as _, bail};
use std::process::Command;

pub struct Editor;

impl Editor {
    const FALLBACK: &'static str = "vi";

    pub const FLAG_TEMPLATE: &'static str = "key: my-flag\ntype: boolean\nenabled: false\ndefault: \"off\"\nvariants:\n  \"on\": true\n  \"off\": false\n";

    pub const SEGMENT_TEMPLATE: &'static str = "key: my-segment\nname: My segment\nconstraints:\n- attribute: plan\n  operator: eq\n  values:\n  - pro\n";

    fn command() -> String {
        std::env::var("VISUAL")
            .ok()
            .or_else(|| std::env::var("EDITOR").ok())
            .filter(|editor| !editor.trim().is_empty())
            .unwrap_or_else(|| Self::FALLBACK.to_owned())
    }

    pub fn edit(initial: &str, name: &str) -> anyhow::Result<String> {
        let dir = std::env::temp_dir().join(format!("ffctl-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, initial)?;

        let command = Self::command();
        let mut parts = command.split_whitespace();
        let program = parts.next().unwrap_or(Self::FALLBACK);

        let status = Command::new(program)
            .args(parts)
            .arg(&path)
            .status()
            .with_context(|| format!("running editor `{command}`"));

        let edited = std::fs::read_to_string(&path);
        let _ = std::fs::remove_dir_all(&dir);

        if !status?.success() {
            bail!("editor `{command}` exited with an error");
        }

        Ok(edited?)
    }
}
