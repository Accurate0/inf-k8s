use anyhow::Context;
use std::path::{Component, Path, PathBuf};

/// Cap on the bytes returned by any single file read.
const MAX_READ_OUTPUT: usize = 60_000;
/// Cap on the number of lines `grep` reports.
const MAX_GREP_MATCHES: usize = 200;

/// A local checkout the model is allowed to explore. All paths are resolved
/// against `root` and traversal outside it is rejected.
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, rel: &str) -> anyhow::Result<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute() || p.components().any(|c| c == Component::ParentDir) {
            anyhow::bail!("refusing unsafe path: {rel}");
        }
        Ok(self.root.join(p))
    }

    pub fn read_file(&self, rel: &str) -> anyhow::Result<String> {
        let abs = self.resolve(rel)?;
        let bytes = std::fs::read(&abs).with_context(|| format!("reading {rel}"))?;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if text.len() > MAX_READ_OUTPUT {
            text.truncate(MAX_READ_OUTPUT);
            text.push_str("\n…(truncated)");
        }
        Ok(text)
    }

    pub fn list_dir(&self, rel: &str) -> anyhow::Result<String> {
        let abs = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            self.resolve(rel)?
        };
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&abs).with_context(|| format!("listing {rel}"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                entries.push(format!("{name}/"));
            } else {
                entries.push(name);
            }
        }
        entries.sort();
        Ok(entries.join("\n"))
    }

    pub fn grep(&self, pattern: &str, path: Option<&str>) -> anyhow::Result<String> {
        let re = regex::Regex::new(pattern).context("invalid regex")?;
        let base = match path {
            Some(p) if !p.is_empty() && p != "." => self.resolve(p)?,
            _ => self.root.clone(),
        };
        let mut out = Vec::new();
        self.grep_walk(&re, &base, &mut out)?;
        if out.is_empty() {
            Ok("(no matches)".to_owned())
        } else {
            Ok(out.join("\n"))
        }
    }

    fn grep_walk(
        &self,
        re: &regex::Regex,
        dir: &Path,
        out: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        if out.len() >= MAX_GREP_MATCHES {
            return Ok(());
        }
        if !dir.is_dir() {
            return self.grep_file(re, dir, out);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            let p = entry.path();
            if entry.file_type()?.is_dir() {
                self.grep_walk(re, &p, out)?;
            } else {
                self.grep_file(re, &p, out)?;
            }
            if out.len() >= MAX_GREP_MATCHES {
                break;
            }
        }
        Ok(())
    }

    fn grep_file(
        &self,
        re: &regex::Regex,
        file: &Path,
        out: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        let Ok(bytes) = std::fs::read(file) else {
            return Ok(());
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(());
        };
        let rel = file.strip_prefix(&self.root).unwrap_or(file).display();
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                out.push(format!("{rel}:{}:{}", i + 1, line.trim_end()));
                if out.len() >= MAX_GREP_MATCHES {
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let abs = dir.path().join(path);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(abs, content).unwrap();
        }
        let ws = Workspace::new(dir.path());
        (dir, ws)
    }

    #[test]
    fn read_file_returns_contents() {
        let (_d, ws) = workspace_with(&[("src/main.rs", "fn main() {}")]);
        assert_eq!(ws.read_file("src/main.rs").unwrap(), "fn main() {}");
    }

    #[test]
    fn read_file_rejects_traversal() {
        let (_d, ws) = workspace_with(&[("a.txt", "x")]);
        assert!(ws.read_file("../etc/passwd").is_err());
    }

    #[test]
    fn list_dir_lists_root() {
        let (_d, ws) = workspace_with(&[("Cargo.toml", ""), ("src/main.rs", "")]);
        let out = ws.list_dir(".").unwrap();
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("src/"));
    }

    #[test]
    fn grep_finds_matches() {
        let (_d, ws) = workspace_with(&[("src/lib.rs", "use serde;\nfn foo() {}\n")]);
        let out = ws.grep("^use ", None).unwrap();
        assert!(out.contains("src/lib.rs:1:use serde;"), "{out}");
    }

    #[test]
    fn grep_no_matches() {
        let (_d, ws) = workspace_with(&[("a.rs", "fn main() {}")]);
        assert_eq!(ws.grep("zzz", None).unwrap(), "(no matches)");
    }
}
