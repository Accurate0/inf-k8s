use std::{ffi::OsString, os::unix::process::CommandExt, process::Command};

use anyhow::{Context, bail};

use crate::{package_auth, session::Settings, store::Store, wasm};

pub async fn exec(settings: &Settings, args: Vec<OsString>) -> anyhow::Result<()> {
    let mut args = args.into_iter();
    let command = args
        .next()
        .context("missing command")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("command is not valid utf-8"))?;

    let store = Store::open()?;
    let installed = store.load()?;

    let mut process = match installed.resolve(&command) {
        Some((name, package)) => {
            let binary = store.current_binary(name, package);
            if !binary.exists() {
                bail!(
                    "{name} is installed but {} is missing; run `ortzi install --force {name}`",
                    binary.display()
                );
            }
            let rest: Vec<OsString> = args.by_ref().collect();
            let mut token = None;
            if let Some(auth) = &package.auth {
                token = match std::env::var(&auth.token_env) {
                    Ok(value) => Some((auth.token_env.clone(), value)),
                    Err(_) if wants_help(&rest) => None,
                    Err(_) => Some((
                        auth.token_env.clone(),
                        package_auth::token(name, auth).await?,
                    )),
                };
            }

            if package.is_wasm() {
                let args = rest
                    .into_iter()
                    .map(|a| a.into_string())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| anyhow::anyhow!("arguments must be valid utf-8"))?;
                let mut vars = vec![("ORTZI_REGISTRY_URL".to_owned(), settings.registry.clone())];
                vars.extend(token);
                let sandbox = wasm::Sandbox::new(
                    name,
                    &package.permissions.clone().unwrap_or_default(),
                    args,
                    vars,
                )?;
                let name = name.clone();
                let code = std::thread::spawn(move || wasm::run(&binary, sandbox))
                    .join()
                    .map_err(|_| anyhow::anyhow!("{name} crashed"))?
                    .with_context(|| format!("running {name}"))?;
                std::process::exit(code);
            }

            let mut process = Command::new(binary);
            process.arg0(&command).env("ORTZI_PACKAGE", name);
            if let Some((key, value)) = token {
                process.env(key, value);
            }
            process.args(rest);
            process
        }
        None => match which(&format!("ortzi-{command}")) {
            Some(path) => Command::new(path),
            None => bail!(
                "unknown command `{command}`; if it's a package, run `ortzi install {command}`"
            ),
        },
    };

    let error = process
        .args(args)
        .env_remove("ORTZI_TOKEN")
        .env("ORTZI", "1")
        .env("ORTZI_REGISTRY_URL", &settings.registry)
        .exec();
    Err(error).with_context(|| format!("running {command}"))
}

fn wants_help(args: &[OsString]) -> bool {
    args.first().is_some_and(|a| a == "help")
        || args
            .iter()
            .take_while(|a| *a != "--")
            .any(|a| a == "--help" || a == "-h" || a == "--version" || a == "-V")
}

pub fn which(binary: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(binary))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_skips_login() {
        assert!(wants_help(&args(&["--help"])));
        assert!(wants_help(&args(&["flag", "list", "-h"])));
        assert!(wants_help(&args(&["help", "flag"])));
        assert!(!wants_help(&args(&["flag", "list"])));
        assert!(!wants_help(&args(&["run", "--", "--help"])));
    }
}
