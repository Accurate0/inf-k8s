mod graphql;

use anyhow::bail;
use clap::{CommandFactory, Parser, Subcommand};
use cli_auth::App;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use cynic::{MutationBuilder, QueryBuilder};
use graph_client::GraphClient;
use serde::Serialize;

use graphql::*;

const APP: App = App {
    name: "ortzi/waf",
    scopes: "openid email profile groups",
    login_hint: "run `ortzi login waf`",
};

#[derive(Parser)]
#[command(name = "wafctl", about = "waf-manager admin CLI, through the graph")]
struct Cli {
    #[arg(long, global = true, help = "Print raw JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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

async fn resolve_block(graph: &GraphClient, target: &str) -> anyhow::Result<String> {
    if !target.contains(['.', ':', '/']) {
        return Ok(target.to_owned());
    }

    let blocks = graph
        .run(BlocksQuery::build(BlocksVariables {
            query: Some(target.to_owned()),
        }))
        .await?
        .waf_blocks;
    let wanted = target.trim();
    let matches: Vec<&Block> = blocks
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

fn print_blocks(blocks: &[Block]) {
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
    let graph = GraphClient::connect(APP).await?;
    let json = cli.json;

    match cli.command {
        Command::Candidates => {
            let c = graph.run(CandidatesQuery::build(())).await?.waf_candidates;
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
            let b = graph
                .run(BlocksQuery::build(BlocksVariables { query }))
                .await?
                .waf_blocks;
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
            let block = graph
                .run(CreateBlock::build(CreateBlockVariables {
                    input: CreateBlockInput {
                        cidr,
                        gateway,
                        reason,
                        ttl_hours: ttl.map(i32::try_from).transpose()?,
                    },
                }))
                .await?
                .waf_create_block;
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
            let name = resolve_block(&graph, &target).await?;
            let deleted = graph
                .run(DeleteBlock::build(NameVariables { name: name.clone() }))
                .await?;
            if json {
                return print_json(&deleted.waf_delete_block);
            }
            println!("unblocked {name}");
        }
        Command::Permanent { target } => {
            let name = resolve_block(&graph, &target).await?;
            let block = graph
                .run(MakeBlockPermanent::build(NameVariables {
                    name: name.clone(),
                }))
                .await?
                .waf_make_block_permanent;
            if json {
                return print_json(&block);
            }
            println!("{} is permanent", block.name);
        }
        Command::Ip { addr, logs } => {
            let ip = graph
                .run(IpQuery::build(IpVariables { addr }))
                .await?
                .waf_ip;
            if json {
                return print_json(&ip);
            }
            println!("{} ({}), window {}", ip.client_ip, ip.cidr, ip.window);
            println!("strikes    {}", ip.strikes);
            println!("protected  {}", or_dash(ip.protected_by.as_deref()));
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
            let page = graph
                .run(AuditQuery::build(AuditVariables {
                    offset: i32::try_from(offset)?,
                    limit: i32::try_from(limit)?,
                }))
                .await?
                .waf_audit;
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
            let w = graph
                .run(WorkflowsQuery::build(WorkflowsVariables { workflow }))
                .await?
                .waf_workflows;
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
                let score = wf.scorecard.as_ref();
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
                let a = graph.run(AllowlistQuery::build(())).await?.waf_allowlist;
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
                let allowlist = graph
                    .run(AddAllowlist::build(AllowlistVariables {
                        cidr: cidr.clone(),
                        note,
                    }))
                    .await?
                    .waf_add_allowlist;
                if json {
                    return print_json(&allowlist);
                }
                println!("allowlisted {cidr}");
            }
            AllowlistAction::Remove { cidr } => {
                let allowlist = graph
                    .run(RemoveAllowlist::build(CidrVariables { cidr: cidr.clone() }))
                    .await?
                    .waf_remove_allowlist;
                if json {
                    return print_json(&allowlist);
                }
                println!("removed {cidr} from the allowlist");
            }
        },
    }

    Ok(())
}
