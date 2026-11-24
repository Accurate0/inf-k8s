use std::path::PathBuf;

use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use registry_api::{HarborAccess, HarborAction};
use serde_json::{Value, json};

use crate::{client::Client, remote};

const ROBOT_PREFIX: &str = "robot$exch-";

fn docker_config() -> anyhow::Result<PathBuf> {
    let dir = match std::env::var_os("DOCKER_CONFIG") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::home_dir()
            .context("no home directory")?
            .join(".docker"),
    };
    Ok(dir.join("config.json"))
}

fn load(path: &PathBuf) -> anyhow::Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.iter().all(u8::is_ascii_whitespace) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not valid json", path.display())),
        Ok(_) => Ok(json!({})),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn robot(config: &Value, host: &str) -> Option<String> {
    let auth = config["auths"][host]["auth"].as_str()?;
    let decoded = String::from_utf8(STANDARD.decode(auth).ok()?).ok()?;
    let (username, _) = decoded.split_once(':')?;
    username
        .starts_with(ROBOT_PREFIX)
        .then(|| username.to_owned())
}

fn parse_spec(spec: &str) -> anyhow::Result<HarborAccess> {
    let Some((project, actions)) = spec.split_once(':') else {
        bail!("{spec} is not <project>:<pull|push>[,…]");
    };

    let actions = actions
        .split(',')
        .map(|action| match action {
            "pull" => Ok(HarborAction::Pull),
            "push" => Ok(HarborAction::Push),
            other => bail!("{other} is not pull or push"),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if project.is_empty() {
        bail!("{spec} has no project");
    }

    Ok(remote::access(project, &actions, None))
}

fn registry_host(registry: &str) -> &str {
    let host = registry
        .split_once("://")
        .map_or(registry, |(_, rest)| rest);
    host.split('/').next().unwrap_or(host)
}

fn cred_helper(config: &Value, host: &str) -> Option<String> {
    match config["credHelpers"][host].as_str() {
        Some(helper) => Some(helper),
        None => config["credsStore"].as_str(),
    }
    .filter(|helper| !helper.is_empty())
    .map(str::to_owned)
}

async fn docker(args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    run(&["docker"], args, stdin).await
}

async fn run(program: &[&str], args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    use tokio::io::AsyncWriteExt;

    let name = program.join(" ");
    let mut child = tokio::process::Command::new(program[0])
        .args(&program[1..])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {name}"))?;

    let mut pipe = child.stdin.take().context("no stdin")?;
    pipe.write_all(stdin.unwrap_or_default().as_bytes()).await?;
    drop(pipe);

    let output = child.wait_with_output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        bail!(
            "{name} {}: {}{}",
            args.join(" "),
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(stdout)
}

async fn current_robot(host: &str) -> anyhow::Result<Option<String>> {
    let config = load(&docker_config()?)?;
    let Some(helper) = cred_helper(&config, host) else {
        return Ok(robot(&config, host));
    };

    let program = format!("docker-credential-{helper}");
    let Ok(stored) = run(&[&program], &["get"], Some(host)).await else {
        return Ok(None);
    };
    let stored: Value = serde_json::from_str(&stored)
        .with_context(|| format!("{program} get returned invalid json"))?;

    Ok(stored["Username"]
        .as_str()
        .filter(|username| username.starts_with(ROBOT_PREFIX))
        .map(str::to_owned))
}

pub async fn login(api: &Client, specs: &[String]) -> anyhow::Result<String> {
    let access = specs
        .iter()
        .map(|s| parse_spec(s))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let token = remote::exchange(api, access).await?;

    match current_robot(&token.registry).await {
        Ok(Some(username)) => {
            if let Err(e) = remote::revoke_robot(api, &username).await {
                eprintln!("warning: could not revoke harbor robot {username}: {e:#}");
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("warning: could not read the current docker login: {e:#}"),
    }

    let login = docker(
        &[
            "login",
            "--username",
            &token.username,
            "--password-stdin",
            &token.registry,
        ],
        Some(&token.password),
    )
    .await;
    if let Err(e) = login {
        remote::revoke(api, &token).await;
        return Err(e);
    }

    if let Some(env) = std::env::var_os("GITHUB_ENV") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(env)?;
        writeln!(file, "HARBOR_ROBOT={}", token.username)?;
    }

    println!(
        "logged in to {} as {} (expires {})",
        token.registry,
        token.username,
        token.expires_at.format("%Y-%m-%d %H:%M UTC")
    );
    Ok(token.registry)
}

pub async fn logout(api: &Client, registry: &str) -> anyhow::Result<()> {
    let host = registry_host(registry);
    let Some(username) = current_robot(host).await? else {
        println!("not logged in to {host}");
        return Ok(());
    };

    remote::revoke_robot(api, &username).await?;
    docker(&["logout", host], None).await?;
    println!("revoked {username} and logged out of {host}");
    Ok(())
}

pub async fn sign(api: &Client, image: &str) -> anyhow::Result<()> {
    let Some((name, digest)) = image.split_once('@') else {
        bail!("{image} is not <repository>@sha256:…");
    };
    let repository = match name.split_once('/') {
        Some((host, rest)) if host.contains('.') || host.contains(':') => rest,
        _ => name,
    };

    let signed = api.sign_image(repository, digest).await?;
    println!(
        "signed {}@{} ({})",
        signed.repository, signed.digest, signed.tag
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse() {
        let access = parse_spec("containers:pull,push").unwrap();
        assert_eq!(access.project, "containers");
        assert_eq!(access.actions, vec![HarborAction::Pull, HarborAction::Push]);
        assert!(parse_spec("containers").is_err());
        assert!(parse_spec(":pull").is_err());
        assert!(parse_spec("containers:delete").is_err());
    }

    #[test]
    fn only_exchange_robots_are_revoked() {
        let auth = |user: &str| STANDARD.encode(format!("{user}:secret"));
        let config = json!({ "auths": {
            "registry.anurag.sh": { "auth": auth("robot$exch-user-anurag-1234abcd") },
            "ghcr.io": { "auth": auth("someone") },
        }});
        assert_eq!(
            robot(&config, "registry.anurag.sh").as_deref(),
            Some("robot$exch-user-anurag-1234abcd")
        );
        assert_eq!(robot(&config, "ghcr.io"), None);
        assert_eq!(robot(&config, "missing"), None);
    }

    #[test]
    fn host_helpers_win_over_the_store() {
        let config = json!({
            "credsStore": "desktop",
            "credHelpers": { "registry.anurag.sh": "pass", "ghcr.io": "" },
        });
        assert_eq!(
            cred_helper(&config, "registry.anurag.sh").as_deref(),
            Some("pass")
        );
        assert_eq!(cred_helper(&config, "quay.io").as_deref(), Some("desktop"));
        assert_eq!(cred_helper(&config, "ghcr.io"), None);
        assert_eq!(cred_helper(&json!({}), "registry.anurag.sh"), None);
    }

    #[test]
    fn registry_hosts() {
        assert_eq!(
            registry_host("https://registry.anurag.sh"),
            "registry.anurag.sh"
        );
        assert_eq!(
            registry_host("https://registry.anurag.sh/"),
            "registry.anurag.sh"
        );
        assert_eq!(registry_host("registry.anurag.sh"), "registry.anurag.sh");
    }
}
