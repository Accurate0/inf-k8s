mod bao;
mod env;
mod kube;

use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::unix::process::CommandExt;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use bao::Bao;
use clap::{CommandFactory, Parser, Subcommand};
use cli_auth::{App, OAuthClient, TokenSource};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use serde_json::{Map, Value};

const APP: App = App {
    name: "secretsctl",
    scopes: "openid email profile",
    login_hint: "run `ortzi login secrets` or set SECRETSCTL_TOKEN",
};

const TOKEN_VARS: [&str; 3] = ["SECRETSCTL_TOKEN", "ORTZI_TOKEN", "BAO_TOKEN"];
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(name = "secretsctl", about = "OpenBao secrets CLI")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "BAO_ADDR",
        default_value = "https://vault.ortzi.cloud"
    )]
    addr: String,

    #[arg(long, global = true, default_value = "kv", help = "KV v2 mount")]
    mount: String,

    #[arg(
        long,
        global = true,
        env = "SECRETSCTL_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/openbao-cli",
        hide_short_help = true
    )]
    issuer: String,

    #[arg(
        long,
        global = true,
        env = "SECRETSCTL_OIDC_CLIENT_ID",
        default_value = "openbao-cli",
        hide_short_help = true
    )]
    client_id: String,

    #[arg(
        long,
        global = true,
        env = "SECRETSCTL_TOKEN",
        hide_env_values = true,
        help = "Kanidm access token to log in to OpenBao with, instead of the stored login"
    )]
    token: Option<String>,

    #[arg(
        long,
        global = true,
        env = "BAO_TOKEN",
        hide_env_values = true,
        hide_short_help = true,
        help = "Use this OpenBao token directly"
    )]
    bao_token: Option<String>,

    #[arg(long, global = true, help = "Print JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in to Kanidm through the browser and store the session
    Login,
    /// Forget the stored session
    Logout,
    /// Show the OpenBao identity and policies you get
    Whoami,
    /// List namespaces, or the secrets under one
    Ls { prefix: Option<String> },
    /// Show a secret's keys (values masked), or one raw value
    Get {
        path: String,
        key: Option<String>,
        #[arg(long, help = "Show values")]
        reveal: bool,
    },
    /// Set keys, reading each value from stdin, then sync its ExternalSecrets
    Set {
        path: String,
        #[arg(required_unless_present = "files")]
        keys: Vec<String>,
        #[arg(long = "file", value_name = "KEY=PATH", help = "Read a key's value from a file")]
        files: Vec<String>,
        #[arg(long)]
        no_sync: bool,
    },
    /// Remove keys, or with --all delete the latest version
    Rm {
        path: String,
        #[arg(required_unless_present = "all")]
        keys: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        no_sync: bool,
    },
    /// Show a secret's versions
    History { path: String },
    /// Force the ExternalSecrets reading a path (or every one in a namespace) to refresh
    Sync { target: String },
    /// Show ExternalSecret sync status, failing first
    Status {
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Print `export KEY='value'` lines for `eval "$(ortzi secrets env ns/app)"`
    Env {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Run a command with secrets in its environment
    Run {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

fn table(header: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY).set_header(header);
    table
}

fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\n', '\r']).to_owned());
    }

    eprint!("{prompt}: ");
    std::io::stderr().flush()?;
    let _ = std::process::Command::new("stty").arg("-echo").status();
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    let _ = std::process::Command::new("stty").arg("echo").status();
    eprintln!();
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_owned())
}

fn confirm(question: &str) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("{question}: pass --yes to confirm non-interactively");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

async fn connect(cli: &Cli, oauth: OAuthClient) -> anyhow::Result<Bao> {
    if let Some(token) = cli.bao_token.clone().filter(|t| !t.is_empty()) {
        return Bao::from_token(&cli.addr, &cli.mount, token);
    }
    let jwt = match cli.token.clone().filter(|t| !t.is_empty()) {
        Some(token) => token,
        None => TokenSource::acquire(oauth).await?.access_token(),
    };
    Bao::login(&cli.addr, &cli.mount, &jwt).await
}

fn sync(target: &str) -> anyhow::Result<bool> {
    let all = kube::list(None)?;
    let found = kube::matching(&all, target);
    if found.is_empty() {
        eprintln!("no ExternalSecrets read {target}");
        return Ok(true);
    }

    let started = chrono::Utc::now();
    let stamp = started.timestamp().to_string();
    for es in &found {
        kube::force_sync(es, &stamp)?;
    }

    let deadline = Instant::now() + SYNC_TIMEOUT;
    let mut pending: Vec<kube::ExternalSecret> = found.into_iter().cloned().collect();
    let mut ok = true;
    while !pending.is_empty() {
        let mut still = Vec::new();
        for es in pending {
            let current = kube::get(&es)?;
            let refreshed = current
                .status
                .refresh_time
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t.timestamp() >= started.timestamp());
            let name = format!("{}/{}", current.metadata.namespace, current.metadata.name);
            match (refreshed, current.is_ready()) {
                (true, true) => println!("synced {name}"),
                (true, false) | (false, false) if Instant::now() >= deadline => {
                    let reason = current
                        .ready()
                        .and_then(|c| c.message.clone().or(c.reason.clone()))
                        .unwrap_or_else(|| "not ready".to_owned());
                    println!("failed {name}: {reason}");
                    ok = false;
                }
                _ if Instant::now() >= deadline => {
                    println!("timed out waiting for {name}");
                    ok = false;
                }
                _ => still.push(current),
            }
        }
        pending = still;
        if !pending.is_empty() {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(ok)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let oauth = OAuthClient::new(APP, &cli.issuer, &cli.client_id);

    match &cli.command {
        Command::Login => return cli_auth::commands::login(&oauth).await,
        Command::Logout => return cli_auth::commands::logout(&oauth),
        Command::Sync { target } => {
            if !sync(target)? {
                std::process::exit(1);
            }
            return Ok(());
        }
        Command::Status { namespace } => {
            let mut all: Vec<_> = kube::list(namespace.as_deref())?
                .into_iter()
                .filter(|es| es.reads_openbao())
                .collect();
            all.sort_by_key(|es| {
                (
                    es.is_ready(),
                    es.metadata.namespace.clone(),
                    es.metadata.name.clone(),
                )
            });
            let mut t = table(&["externalsecret", "key", "ready", "reason", "refreshed"]);
            for es in &all {
                let ready = es.ready();
                t.add_row([
                    format!("{}/{}", es.metadata.namespace, es.metadata.name),
                    es.keys().collect::<Vec<_>>().join(", "),
                    ready.map_or("?", |c| c.status.as_str()).to_owned(),
                    ready
                        .and_then(|c| c.reason.clone())
                        .unwrap_or_default(),
                    es.status.refresh_time.clone().unwrap_or_default(),
                ]);
            }
            println!("{t}");
            return Ok(());
        }
        _ => {}
    }

    let bao = connect(&cli, oauth).await?;

    match cli.command {
        Command::Login | Command::Logout | Command::Sync { .. } | Command::Status { .. } => {
            unreachable!()
        }
        Command::Whoami => {
            let me = bao.whoami().await?;
            println!("user      {}", me.display_name);
            println!("policies  {}", me.policies.join(", "));
            println!("expires   in {}s", me.ttl);
            println!("openbao   {}", cli.addr);
        }
        Command::Ls { prefix } => {
            let keys = bao.list(prefix.as_deref().unwrap_or("")).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&keys)?);
            } else {
                for key in keys {
                    match &prefix {
                        Some(p) => println!("{}/{key}", p.trim_matches('/')),
                        None => println!("{key}"),
                    }
                }
            }
        }
        Command::Get { path, key, reveal } => {
            let secret = bao.require(&path).await?;
            if let Some(key) = key {
                let value = secret
                    .data
                    .get(&key)
                    .with_context(|| format!("{path} has no key {key}"))?;
                println!("{}", display(value));
                return Ok(());
            }
            if cli.json {
                let data: Map<String, Value> = if reveal {
                    secret.data
                } else {
                    secret
                        .data
                        .keys()
                        .map(|k| (k.clone(), Value::String("********".into())))
                        .collect()
                };
                println!("{}", serde_json::to_string_pretty(&data)?);
                return Ok(());
            }
            let mut t = table(&["key", "value"]);
            for (k, v) in &secret.data {
                t.add_row([
                    k.clone(),
                    if reveal {
                        display(v)
                    } else {
                        "********".to_owned()
                    },
                ]);
            }
            println!("{t}");
            println!(
                "version {} ({})",
                secret.metadata.version, secret.metadata.created_time
            );
        }
        Command::Set {
            path,
            keys,
            files,
            no_sync,
        } => {
            let mut changes = Map::new();
            for key in &keys {
                if !env::is_name(key) && key.contains(['=', ' ']) {
                    bail!("pass key names only; values are read from stdin");
                }
                changes.insert(key.clone(), Value::String(read_secret(key)?));
            }
            for file in &files {
                let (key, file) = file
                    .split_once('=')
                    .context("--file takes KEY=PATH")?;
                let mut contents = String::new();
                std::fs::File::open(file)
                    .with_context(|| format!("opening {file}"))?
                    .read_to_string(&mut contents)?;
                changes.insert(key.to_owned(), Value::String(contents));
            }
            let version = bao.patch(&path, changes).await?;
            println!("wrote {path} version {version}");
            if !no_sync && !sync(&path)? {
                std::process::exit(1);
            }
        }
        Command::Rm {
            path,
            keys,
            all,
            yes,
            no_sync,
        } => {
            if all {
                if !yes && !confirm(&format!("delete the latest version of {path}?"))? {
                    bail!("cancelled");
                }
                bao.delete_latest(&path).await?;
                println!("deleted the latest version of {path}");
            } else {
                let secret = bao.require(&path).await?;
                for key in &keys {
                    if !secret.data.contains_key(key) {
                        bail!("{path} has no key {key}");
                    }
                }
                let changes = keys.iter().map(|k| (k.clone(), Value::Null)).collect();
                let version = bao.patch(&path, changes).await?;
                println!("removed {} from {path} (version {version})", keys.join(", "));
            }
            if !no_sync && !sync(&path)? {
                std::process::exit(1);
            }
        }
        Command::History { path } => {
            let meta = bao.metadata(&path).await?;
            let mut versions: Vec<_> = meta.versions.into_iter().collect();
            versions.sort_by_key(|(v, _)| v.parse::<u64>().unwrap_or_default());
            let mut t = table(&["version", "created", "deleted", ""]);
            for (number, v) in versions.iter().rev() {
                t.add_row([
                    number.clone(),
                    v.created_time.clone(),
                    v.deletion_time.clone(),
                    if v.destroyed {
                        "destroyed".to_owned()
                    } else if number.parse::<u64>().ok() == Some(meta.current_version) {
                        "current".to_owned()
                    } else {
                        String::new()
                    },
                ]);
            }
            println!("{t}");
        }
        Command::Env { paths, prefix } => {
            let mut secrets = Vec::new();
            for path in &paths {
                secrets.push((path.clone(), bao.require(path).await?.data));
            }
            print!("{}", env::exports(&env::collect(secrets, &prefix)));
        }
        Command::Run {
            paths,
            prefix,
            command,
        } => {
            let mut secrets = Vec::new();
            for path in &paths {
                secrets.push((path.clone(), bao.require(path).await?.data));
            }
            let vars = env::collect(secrets, &prefix);
            let (program, args) = command.split_first().context("missing command")?;
            let mut child = std::process::Command::new(program);
            child.args(args).envs(&vars);
            for var in TOKEN_VARS {
                child.env_remove(var);
            }
            let error = child.exec();
            return Err(error).with_context(|| format!("running {program}"));
        }
    }

    Ok(())
}
