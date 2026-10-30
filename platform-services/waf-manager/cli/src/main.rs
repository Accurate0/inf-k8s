use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand};
use cli_auth::{App, OAuthClient, TokenSource};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use waf_manager_api as api;

const APP: App = App {
    name: "wafctl",
    scopes: "openid email profile",
    login_hint: "run `ortzi login waf` or set WAFCTL_TOKEN",
};

#[derive(Parser)]
#[command(name = "wafctl", about = "waf-manager admin CLI")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "WAFCTL_URL",
        default_value = "https://waf-manager.ortzi.cloud"
    )]
    url: String,

    #[arg(
        long,
        global = true,
        env = "WAFCTL_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/waf-manager-cli",
        hide_short_help = true
    )]
    issuer: String,

    #[arg(
        long,
        global = true,
        env = "WAFCTL_OIDC_CLIENT_ID",
        default_value = "waf-manager-cli",
        hide_short_help = true
    )]
    client_id: String,

    #[arg(
        long,
        global = true,
        env = "WAFCTL_TOKEN",
        hide_env_values = true,
        help = "Use this bearer token instead of the stored login"
    )]
    token: Option<String>,

    #[arg(long, global = true, help = "Print raw JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in through the browser and store the session
    Login,
    /// Forget the stored session
    Logout,
    /// Show the logged in user
    Whoami,
    /// Client IPs with WAF detections in the current window
    Candidates,
    /// List blocks, optionally filtered by CIDR substring
    Blocks {
        #[arg(short, long)]
        query: Option<String>,
    },
    /// Block a CIDR or address
    Block {
        cidr: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(
            long,
            help = "Expire the block after this many hours; omit for permanent"
        )]
        ttl: Option<u32>,
        #[arg(long)]
        gateway: Option<String>,
    },
    /// Remove a block, by name or CIDR
    Unblock { target: String },
    /// Make an expiring block permanent, by name or CIDR
    Permanent { target: String },
    /// Show detections, history and enrichment for an address
    Ip {
        addr: String,
        #[arg(long, help = "Also print recent raw log lines")]
        logs: bool,
    },
    /// Show the audit log
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    /// Show workflows, their scorecard and recent decisions
    Workflows {
        #[arg(long, help = "Only show decisions from this workflow")]
        workflow: Option<String>,
    },
    /// Manage the manual allowlist
    Allowlist {
        #[command(subcommand)]
        action: Option<AllowlistAction>,
    },
}

#[derive(Subcommand)]
enum AllowlistAction {
    /// List manual entries and protected sources
    List,
    /// Allowlist a CIDR or address
    Add {
        cidr: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Remove a manual allowlist entry
    Remove { cidr: String },
}

struct Client {
    base: String,
    http: reqwest::Client,
    bearer: String,
}

impl Client {
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> anyhow::Result<Option<T>> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.bearer);
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await.context("calling waf-manager")?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() && status != StatusCode::NOT_MODIFIED {
            bail!("waf-manager returned {status}: {}", text.trim());
        }
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&text).with_context(|| {
            format!("unexpected response from {path}")
        })?))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.request(Method::GET, path, None::<&()>)
            .await?
            .with_context(|| format!("empty response from {path}"))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> anyhow::Result<()> {
        self.request::<serde_json::Value>(method, path, body)
            .await?;
        Ok(())
    }

    async fn resolve_block(&self, target: &str) -> anyhow::Result<String> {
        if !target.contains(['.', ':', '/']) {
            return Ok(target.to_owned());
        }
        let blocks: api::Blocks = self
            .get(&format!("/api/v1/blocks?q={}", encode(target)))
            .await?;
        let wanted = target.trim();
        let matches: Vec<&api::Block> = blocks
            .blocks
            .iter()
            .filter(|b| {
                b.cidr == wanted
                    || b.cidr.strip_suffix("/32") == Some(wanted)
                    || b.cidr.strip_suffix("/128") == Some(wanted)
            })
            .collect();
        match matches.as_slice() {
            [only] => Ok(only.name.clone()),
            [] => bail!("no block for {target}"),
            _ => bail!("several blocks match {target}; pass the block name"),
        }
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn table(header: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY).set_header(header);
    table
}

fn or_dash(value: Option<&str>) -> &str {
    value.filter(|v| !v.is_empty()).unwrap_or("-")
}

fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn short(stamp: &chrono::DateTime<chrono::Utc>) -> String {
    stamp.format("%Y-%m-%d %H:%M").to_string()
}

fn print_blocks(blocks: &[api::Block]) {
    let mut t = table(&[
        "name", "cidr", "reason", "by", "expires", "strikes", "enforced",
    ]);
    for b in blocks {
        t.add_row([
            b.name.clone(),
            b.cidr.clone(),
            or_dash(b.reason.as_deref()).to_owned(),
            format!(
                "{}{}",
                or_dash(b.created_by.as_deref()),
                if b.automatic { " (auto)" } else { "" }
            ),
            or_dash(b.expires_at.as_deref()).to_owned(),
            b.strikes.to_string(),
            if b.enforced { "yes" } else { "no" }.to_owned(),
        ]);
    }
    println!("{t}");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let oauth = OAuthClient::new(APP, &cli.issuer, &cli.client_id);
    let base = cli.url.trim_end_matches('/').to_owned();

    match &cli.command {
        Command::Login => return cli_auth::commands::login(&oauth).await,
        Command::Logout => return cli_auth::commands::logout(&oauth),
        Command::Whoami => return cli_auth::commands::whoami(&oauth, &base),
        _ => {}
    }

    let bearer = match cli.token.clone().filter(|t| !t.is_empty()) {
        Some(token) => token,
        None => TokenSource::acquire(oauth).await?.access_token(),
    };
    let client = Client {
        base,
        http: reqwest::Client::builder()
            .user_agent(concat!("wafctl/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .build()?,
        bearer,
    };
    let json = cli.json;

    match cli.command {
        Command::Login | Command::Logout | Command::Whoami => unreachable!(),
        Command::Candidates => {
            let c: api::Candidates = client.get("/api/v1/candidates").await?;
            if json {
                return print_json(&c);
            }
            println!("window {}", c.window);
            let mut t = table(&["client ip", "detections", "state"]);
            for row in &c.actionable {
                t.add_row([
                    row.client_ip.clone(),
                    row.detections.to_string(),
                    "actionable".into(),
                ]);
            }
            for row in &c.protected {
                t.add_row([
                    row.client_ip.clone(),
                    row.detections.to_string(),
                    format!("protected: {}", or_dash(row.protected_by.as_deref())),
                ]);
            }
            for row in &c.blocked {
                t.add_row([
                    row.client_ip.clone(),
                    row.detections.to_string(),
                    "blocked".into(),
                ]);
            }
            println!("{t}");
        }
        Command::Blocks { query } => {
            let path = match query {
                Some(q) => format!("/api/v1/blocks?q={}", encode(&q)),
                None => "/api/v1/blocks".to_owned(),
            };
            let b: api::Blocks = client.get(&path).await?;
            if json {
                return print_json(&b);
            }
            print_blocks(&b.blocks);
            for conflict in &b.conflicts {
                eprintln!("conflict ({}): {}", conflict.source, conflict.message);
            }
        }
        Command::Block {
            cidr,
            reason,
            ttl,
            gateway,
        } => {
            let block: Option<api::Block> = client
                .request(
                    Method::POST,
                    "/api/v1/blocks",
                    Some(&api::CreateBlock {
                        cidr,
                        gateway,
                        reason,
                        ttl_hours: ttl,
                    }),
                )
                .await?;
            let block = block.context("waf-manager returned no block")?;
            if json {
                return print_json(&block);
            }
            println!(
                "blocked {} as {} ({})",
                block.cidr,
                block.name,
                block
                    .expires_at
                    .map(|at| format!("expires {at}"))
                    .unwrap_or_else(|| "permanent".to_owned())
            );
        }
        Command::Unblock { target } => {
            let name = client.resolve_block(&target).await?;
            client
                .send(
                    Method::DELETE,
                    &format!("/api/v1/blocks/{name}"),
                    None::<&()>,
                )
                .await?;
            println!("unblocked {name}");
        }
        Command::Permanent { target } => {
            let name = client.resolve_block(&target).await?;
            client
                .send(
                    Method::POST,
                    &format!("/api/v1/blocks/{name}/permanent"),
                    None::<&()>,
                )
                .await?;
            println!("{name} is permanent");
        }
        Command::Ip { addr, logs } => {
            let ip: api::IpDetail = client.get(&format!("/api/v1/ip/{}", encode(&addr))).await?;
            if json {
                return print_json(&ip);
            }
            println!("{} ({}), window {}", ip.client_ip, ip.cidr, ip.window);
            println!("strikes    {}", ip.strikes);
            println!("protected  {}", or_dash(ip.protected.as_deref()));
            if let Some(e) = &ip.enrichment {
                println!("hostname   {}", or_dash(e.hostname.as_deref()));
                println!(
                    "network    {} {} {} {}",
                    e.asn
                        .as_deref()
                        .map(|a| format!("AS{a}"))
                        .unwrap_or_else(|| "-".into()),
                    or_dash(e.org.as_deref()),
                    or_dash(e.prefix.as_deref()),
                    or_dash(e.country.as_deref())
                );
            }
            match &ip.block {
                Some(block) => print_blocks(std::slice::from_ref(block)),
                None => println!("block      none"),
            }
            if !ip.rules.is_empty() {
                let mut t = table(&["rule", "severity", "hits", "message"]);
                for r in &ip.rules {
                    t.add_row([
                        format!("{}{}", r.rule_id, if r.ignored { " (ignored)" } else { "" }),
                        r.severity.clone(),
                        r.count.to_string(),
                        r.rule_msg.clone(),
                    ]);
                }
                println!("{t}");
            }
            if !ip.uris.is_empty() {
                let mut t = table(&["uri", "hits"]);
                for u in &ip.uris {
                    t.add_row([u.uri.clone(), u.count.to_string()]);
                }
                println!("{t}");
            }
            if !ip.decisions.is_empty() {
                let mut t = table(&["at", "workflow", "mode", "outcome", "detections"]);
                for d in &ip.decisions {
                    t.add_row([
                        short(&d.at),
                        d.workflow.clone(),
                        d.mode.clone(),
                        d.outcome.clone(),
                        d.detections.to_string(),
                    ]);
                }
                println!("{t}");
            }
            if !ip.audit.is_empty() {
                let mut t = table(&["at", "actor", "action", "detail"]);
                for a in &ip.audit {
                    t.add_row([
                        short(&a.at),
                        a.actor.clone(),
                        a.action.clone(),
                        or_dash(a.detail.as_deref()).to_owned(),
                    ]);
                }
                println!("{t}");
            }
            if logs {
                for line in &ip.lines {
                    println!("{line}");
                }
            }
        }
        Command::Audit { limit, offset } => {
            let page: api::AuditPage = client
                .get(&format!("/api/v1/audit?limit={limit}&offset={offset}"))
                .await?;
            if json {
                return print_json(&page);
            }
            let mut t = table(&["at", "actor", "action", "target", "detail"]);
            for a in &page.entries {
                t.add_row([
                    short(&a.at),
                    a.actor.clone(),
                    a.action.clone(),
                    a.target.clone(),
                    or_dash(a.detail.as_deref()).to_owned(),
                ]);
            }
            println!("{t}");
            println!(
                "showing {}-{} of {}",
                offset + 1,
                offset + page.entries.len() as i64,
                page.total
            );
        }
        Command::Workflows { workflow } => {
            let path = match workflow {
                Some(w) => format!("/api/v1/workflows?workflow={}", encode(&w)),
                None => "/api/v1/workflows".to_owned(),
            };
            let w: api::Workflows = client.get(&path).await?;
            if json {
                return print_json(&w);
            }
            let mut t = table(&[
                "workflow",
                "enabled",
                "tier",
                "window",
                "duration",
                "decisions",
                "unblocked",
            ]);
            for wf in &w.workflows {
                let score = w.scorecard.iter().find(|s| s.workflow == wf.name);
                t.add_row([
                    wf.name.clone(),
                    wf.enabled.clone(),
                    wf.tier.clone(),
                    wf.window.clone(),
                    wf.duration.clone(),
                    score.map(|s| s.decisions.to_string()).unwrap_or_default(),
                    score.map(|s| s.unblocked.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{t}");
            if !w.decisions.is_empty() {
                let mut t = table(&["at", "workflow", "cidr", "mode", "outcome"]);
                for d in &w.decisions {
                    t.add_row([
                        short(&d.at),
                        d.workflow.clone(),
                        d.cidr.clone(),
                        d.mode.clone(),
                        d.outcome.clone(),
                    ]);
                }
                println!("{t}");
            }
            println!("manual unblock cooldown {}", w.cooldown);
        }
        Command::Allowlist { action } => match action.unwrap_or(AllowlistAction::List) {
            AllowlistAction::List => {
                let a: api::Allowlist = client.get("/api/v1/allowlist").await?;
                if json {
                    return print_json(&a);
                }
                let mut t = table(&["cidr", "note", "by", "at"]);
                for m in &a.manual {
                    t.add_row([
                        m.cidr.clone(),
                        or_dash(m.note.as_deref()).to_owned(),
                        m.created_by.clone(),
                        short(&m.at),
                    ]);
                }
                println!("{t}");
                for p in &a.protected {
                    println!("protected by {}: {} ranges", p.source, p.count);
                }
            }
            AllowlistAction::Add { cidr, note } => {
                client
                    .send(
                        Method::POST,
                        "/api/v1/allowlist",
                        Some(&api::CreateAllowlist {
                            cidr: cidr.clone(),
                            note,
                        }),
                    )
                    .await?;
                println!("allowlisted {cidr}");
            }
            AllowlistAction::Remove { cidr } => {
                client
                    .send(
                        Method::DELETE,
                        &format!("/api/v1/allowlist?cidr={}", encode(&cidr)),
                        None::<&()>,
                    )
                    .await?;
                println!("removed {cidr} from the allowlist");
            }
        },
    }

    Ok(())
}
