use std::{
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
};

use clap::{Arg, CommandFactory};
use clap_complete::{ArgValueCandidates, ArgValueCompleter, CompleteEnv, CompletionCandidate};
use registry_api::Completions;

use crate::{
    Cli,
    store::{Installed, InstalledPackage, Store},
};

const VAR: &str = "COMPLETE";
const INDEX_VAR: &str = "_CLAP_COMPLETE_INDEX";
const NAME_COMMANDS: [(&str, &str); 7] = [
    ("uninstall", "packages"),
    ("update", "packages"),
    ("login", "package"),
    ("logout", "package"),
    ("rollback", "package"),
    ("pin", "package"),
    ("unpin", "package"),
];

pub fn print_script(shell: &str) -> anyhow::Result<()> {
    let status = Command::new(std::env::current_exe()?)
        .env(VAR, shell)
        .status()?;
    anyhow::ensure!(status.success(), "generating {shell} completions failed");
    Ok(())
}

pub fn run() {
    if std::env::var_os(VAR).is_none_or(|v| v.is_empty() || v == "0") {
        return;
    }

    if let Some((store, installed)) = load()
        && let Some(words) = shell_words()
        && let Some((position, name, package)) = package_word(&words, &installed)
        && package.completions == Some(Completions::Clap)
        && !package.is_wasm()
    {
        let index = std::env::var(INDEX_VAR)
            .ok()
            .and_then(|i| i.parse::<usize>().ok());
        if index.is_none_or(|i| i > position) {
            let mut process = Command::new(store.current_binary(name, package));
            process
                .arg0(&words[position])
                .arg("--")
                .args(&words[position..]);
            if let Some(index) = index {
                process.env(INDEX_VAR, (index - position).to_string());
            }
            let _ = process.exec();
            std::process::exit(1);
        }
    }

    CompleteEnv::with_factory(command)
        .bin("ortzi")
        .completer("ortzi")
        .complete();
}

fn load() -> Option<(Store, Installed)> {
    let store = Store::open().ok()?;
    let installed = store.load().ok()?;
    Some((store, installed))
}

fn shell_words() -> Option<Vec<OsString>> {
    let mut args = std::env::args_os().skip_while(|a| a != "--");
    args.next()?;
    Some(args.collect())
}

fn package_word<'a>(
    words: &[OsString],
    installed: &'a Installed,
) -> Option<(usize, &'a String, &'a InstalledPackage)> {
    let (position, word) = words
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, w)| !w.to_string_lossy().starts_with('-'))?;
    let (name, package) = installed.resolve(&word.to_string_lossy())?;
    Some((position, name, package))
}

fn command() -> clap::Command {
    let mut command = Cli::command();
    let Some((store, installed)) = load() else {
        return command;
    };

    let names: Vec<String> = installed.packages.keys().cloned().collect();
    for (subcommand, arg) in NAME_COMMANDS {
        let names = names.clone();
        command = command.mut_subcommand(subcommand, |c| {
            c.mut_arg(arg, |a| {
                a.add(ArgValueCandidates::new(move || {
                    names
                        .iter()
                        .map(|n| CompletionCandidate::new(n.as_str()))
                        .collect()
                }))
            })
        });
    }

    let builtin: Vec<String> = command
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    for (name, package) in &installed.packages {
        if builtin.contains(name) {
            continue;
        }
        let mut subcommand = clap::Command::new(name.clone())
            .visible_aliases(package.aliases.clone())
            .disable_help_flag(true)
            .disable_help_subcommand(true);
        if package.completions == Some(Completions::Cobra) {
            let (program, prefix) = if package.is_wasm() {
                let exe = std::env::current_exe().unwrap_or_else(|_| "ortzi".into());
                (exe, vec![name.clone()])
            } else {
                (store.current_binary(name, package), Vec::new())
            };
            subcommand = subcommand.arg(
                Arg::new("args")
                    .num_args(0..)
                    .trailing_var_arg(true)
                    .allow_hyphen_values(true)
                    .add(ArgValueCompleter::new(move |current: &OsStr| {
                        cobra_candidates(&program, &prefix, current)
                    })),
            );
        }
        command = command.subcommand(subcommand);
    }
    command
}

fn cobra_candidates(
    program: &std::path::Path,
    prefix: &[String],
    current: &OsStr,
) -> Vec<CompletionCandidate> {
    let Some(words) = shell_words() else {
        return Vec::new();
    };
    let Some(position) = words
        .iter()
        .skip(1)
        .position(|w| !w.to_string_lossy().starts_with('-'))
        .map(|p| p + 1)
    else {
        return Vec::new();
    };
    let end = std::env::var(INDEX_VAR)
        .ok()
        .and_then(|i| i.parse().ok())
        .unwrap_or(words.len().saturating_sub(1))
        .clamp(position + 1, words.len());

    let Ok(output) = Command::new(program)
        .env_remove(VAR)
        .args(prefix)
        .arg("__complete")
        .args(&words[position + 1..end])
        .arg(current)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    parse_cobra(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(|(value, help)| {
            let candidate = CompletionCandidate::new(value);
            match help {
                Some(help) => candidate.help(Some(help.to_owned().into())),
                None => candidate,
            }
        })
        .collect()
}

fn parse_cobra(output: &str) -> Vec<(&str, Option<&str>)> {
    output
        .lines()
        .take_while(|line| !line.starts_with(':'))
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once('\t') {
            Some((value, help)) => (value, Some(help)),
            None => (line, None),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use registry_api::Target;

    fn package() -> InstalledPackage {
        InstalledPackage {
            version: "1.0.0".to_owned(),
            target: Target::LinuxAmd64,
            sha256: String::new(),
            binary: "secretsctl".to_owned(),
            aliases: vec!["sec".to_owned()],
            auth: None,
            permissions: None,
            completions: Some(Completions::Clap),
            pinned: false,
            previous: None,
        }
    }

    fn words(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn finds_the_package_word() {
        let mut installed = Installed::default();
        installed.packages.insert("secrets".to_owned(), package());

        let found = |list: &[&str]| package_word(&words(list), &installed).map(|f| f.0);
        assert_eq!(found(&["ortzi", "secrets", "get"]), Some(1));
        assert_eq!(found(&["ortzi", "--flag", "sec", ""]), Some(2));
        assert_eq!(found(&["ortzi", "install", "secrets"]), None);
        assert_eq!(found(&["ortzi", ""]), None);
    }

    #[test]
    fn parses_cobra_output() {
        let output = "app\tManage apps\ncluster\n:4\nCompletion ended with directive\n";
        assert_eq!(
            parse_cobra(output),
            vec![("app", Some("Manage apps")), ("cluster", None)]
        );
        assert!(parse_cobra(":0\n").is_empty());
    }

    #[test]
    fn adding_package_subcommands_keeps_the_cli_valid() {
        command().debug_assert();
    }
}
