use std::path::PathBuf;

use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use registry_api::{HarborAccess, HarborAction, HarborSignRequest, HarborSignature};
use reqwest::Method;
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

fn save(path: &PathBuf, config: &Value) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(config)?)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }

    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
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

async fn revoke_existing(api: &Client, config: &Value, host: &str) {
    if let Some(username) = robot(config, host)
        && let Err(e) = remote::revoke_robot(api, &username).await
    {
        eprintln!("warning: could not revoke harbor robot {username}: {e:#}");
    }
}

pub async fn login(api: &Client, specs: &[String]) -> anyhow::Result<()> {
    let access = specs
        .iter()
        .map(|s| parse_spec(s))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let Some(token) = remote::exchange(api, access).await? else {
        bail!("the registry has no harbor token exchange");
    };

    let path = docker_config()?;
    let mut config = match load(&path) {
        Ok(config) => config,
        Err(e) => {
            remote::revoke(api, &token).await;
            return Err(e);
        }
    };
    revoke_existing(api, &config, &token.registry).await;

    let auth = STANDARD.encode(format!("{}:{}", token.username, token.password));
    if !config["auths"].is_object() {
        config["auths"] = json!({});
    }
    config["auths"][&token.registry] = json!({ "auth": auth });
    if let Err(e) = save(&path, &config) {
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
    Ok(())
}

pub async fn logout(api: &Client, registry: &str) -> anyhow::Result<()> {
    let host = registry_host(registry);
    let path = docker_config()?;
    let mut config = load(&path)?;

    let Some(username) = robot(&config, host) else {
        println!("not logged in to {host}");
        return Ok(());
    };
    remote::revoke_robot(api, &username).await?;

    if let Some(auths) = config["auths"].as_object_mut() {
        auths.remove(host);
    }
    save(&path, &config)?;
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

    let signed: HarborSignature = api
        .send(
            Method::POST,
            "/api/v1/harbor/sign",
            &HarborSignRequest {
                repository: repository.to_owned(),
                digest: digest.to_owned(),
            },
        )
        .await?;
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
