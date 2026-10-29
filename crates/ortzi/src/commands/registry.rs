use clap::Subcommand;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use registry_api::{
    AuditEntry, CreatePackage, CreateToken, CreatedToken, ECOSYSTEM_CLI, NewTrustedPublisher,
    Package, Scope, TokenInfo, TrustedPublisher, UpdatePackage, Version, Yank,
};
use reqwest::Method;

use super::packages::urlencode;
use crate::{client::Client, session::Settings};

#[derive(Subcommand)]
pub enum RegistryCommand {
    /// Create, edit or delete packages
    Package {
        #[command(subcommand)]
        action: PackageAction,
    },
    /// Manage Forgejo repositories trusted to publish a package
    Publisher {
        #[command(subcommand)]
        action: PublisherAction,
    },
    /// Manage publish tokens
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Yank or unyank published versions
    Version {
        #[command(subcommand)]
        action: VersionAction,
    },
    /// Show the audit log
    Audit {
        #[arg(long)]
        package: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

#[derive(clap::Args)]
pub struct TrustArgs {
    #[arg(long = "forgejo-repo", value_name = "OWNER/REPO")]
    pub repository: Option<String>,
    #[arg(long = "ref", value_name = "GLOB", requires = "repository")]
    pub ref_pattern: Option<String>,
    #[arg(long, value_name = "FILE", requires = "repository")]
    pub workflow: Option<String>,
}

#[derive(Subcommand)]
pub enum PackageAction {
    /// Reserve a package name, optionally trusting a Forgejo repo to publish it
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long)]
        binary: Option<String>,
        #[arg(long = "alias")]
        aliases: Vec<String>,
        #[command(flatten)]
        trust: TrustArgs,
    },
    /// Change a package's description, binary or aliases
    Edit {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        binary: Option<String>,
        #[arg(long = "alias")]
        aliases: Option<Vec<String>>,
    },
    /// Delete a package that has never been published
    Delete { name: String },
}

#[derive(Subcommand)]
pub enum PublisherAction {
    /// Trust a Forgejo repository to publish a package via Actions OIDC
    Add {
        package: String,
        #[command(flatten)]
        trust: TrustArgs,
    },
    List {
        package: Option<String>,
    },
    Rm {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum TokenAction {
    /// Mint a publish token scoped to packages; it is shown once
    Create {
        #[arg(long = "package", required = true)]
        packages: Vec<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        expires_in_days: Option<i64>,
    },
    List,
    Revoke {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum VersionAction {
    Yank {
        package: String,
        version: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Unyank {
        package: String,
        version: String,
    },
}

pub async fn run(settings: &Settings, command: RegistryCommand) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    match command {
        RegistryCommand::Package { action } => package(&client, action).await,
        RegistryCommand::Publisher { action } => publisher(&client, action).await,
        RegistryCommand::Token { action } => token(&client, action).await,
        RegistryCommand::Version { action } => version(&client, action).await,
        RegistryCommand::Audit { package, limit } => audit(&client, package, limit).await,
    }
}

async fn add_publisher(client: &Client, package: &str, trust: TrustArgs) -> anyhow::Result<()> {
    let Some(repository) = trust.repository else {
        anyhow::bail!("--forgejo-repo is required");
    };
    let publisher: TrustedPublisher = client
        .send(
            Method::POST,
            "/api/v1/trusted-publishers",
            &NewTrustedPublisher {
                ecosystem: ECOSYSTEM_CLI.to_owned(),
                package: package.to_owned(),
                repository,
                ref_pattern: trust.ref_pattern,
                workflow: trust.workflow,
            },
        )
        .await?;
    println!(
        "trusted {} ({}{}) to publish {package} [{}]",
        publisher.repository,
        publisher.ref_pattern.as_deref().unwrap_or("any ref"),
        publisher
            .workflow
            .map(|w| format!(", {w}"))
            .unwrap_or_default(),
        publisher.id
    );
    Ok(())
}

async fn package(client: &Client, action: PackageAction) -> anyhow::Result<()> {
    match action {
        PackageAction::Create {
            name,
            description,
            binary,
            aliases,
            trust,
        } => {
            let package: Package = client
                .send(
                    Method::POST,
                    "/cli/v1/packages",
                    &CreatePackage {
                        name: name.clone(),
                        description,
                        binary_name: binary,
                        aliases,
                    },
                )
                .await?;
            println!("created {} (binary {})", package.name, package.binary_name);
            if trust.repository.is_some() {
                add_publisher(client, &name, trust).await?;
            }
        }
        PackageAction::Edit {
            name,
            description,
            binary,
            aliases,
        } => {
            let package: Package = client
                .send(
                    Method::PATCH,
                    &format!("/cli/v1/packages/{name}"),
                    &UpdatePackage {
                        description,
                        binary_name: binary,
                        aliases,
                    },
                )
                .await?;
            println!("updated {}", package.name);
        }
        PackageAction::Delete { name } => {
            client.delete(&format!("/cli/v1/packages/{name}")).await?;
            println!("deleted {name}");
        }
    }
    Ok(())
}

async fn publisher(client: &Client, action: PublisherAction) -> anyhow::Result<()> {
    match action {
        PublisherAction::Add { package, trust } => add_publisher(client, &package, trust).await?,
        PublisherAction::List { package } => {
            let path = match package {
                Some(p) => format!("/api/v1/trusted-publishers?package={}", urlencode(&p)),
                None => "/api/v1/trusted-publishers".to_owned(),
            };
            let publishers: Vec<TrustedPublisher> = client.get(&path).await?;
            let mut table = Table::new();
            table.load_preset(UTF8_BORDERS_ONLY).set_header([
                "id",
                "package",
                "repository",
                "ref",
                "workflow",
            ]);
            for p in publishers {
                table.add_row([
                    p.id.to_string(),
                    format!("{}/{}", p.ecosystem, p.package),
                    p.repository,
                    p.ref_pattern.unwrap_or_else(|| "*".to_owned()),
                    p.workflow.unwrap_or_else(|| "*".to_owned()),
                ]);
            }
            println!("{table}");
        }
        PublisherAction::Rm { id } => {
            client
                .delete(&format!("/api/v1/trusted-publishers/{id}"))
                .await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn token(client: &Client, action: TokenAction) -> anyhow::Result<()> {
    match action {
        TokenAction::Create {
            packages,
            name,
            expires_in_days,
        } => {
            let created: CreatedToken = client
                .send(
                    Method::POST,
                    "/api/v1/tokens",
                    &CreateToken {
                        name: name.unwrap_or_else(|| packages.join("+")),
                        scopes: packages
                            .into_iter()
                            .map(|package| Scope {
                                ecosystem: ECOSYSTEM_CLI.to_owned(),
                                package,
                            })
                            .collect(),
                        expires_in_days,
                    },
                )
                .await?;
            eprintln!(
                "token {} (shown once; store it as ORTZI_TOKEN):",
                created.id
            );
            println!("{}", created.token);
        }
        TokenAction::List => {
            let tokens: Vec<TokenInfo> = client.get("/api/v1/tokens").await?;
            let mut table = Table::new();
            table.load_preset(UTF8_BORDERS_ONLY).set_header([
                "id",
                "name",
                "prefix",
                "scopes",
                "last used",
                "state",
            ]);
            for t in tokens {
                let state = match (t.revoked_at, t.expires_at) {
                    (Some(_), _) => "revoked".to_owned(),
                    (None, Some(at)) => format!("expires {}", at.format("%Y-%m-%d")),
                    (None, None) => "active".to_owned(),
                };
                table.add_row([
                    t.id.to_string(),
                    t.name,
                    t.prefix,
                    t.scopes
                        .iter()
                        .map(|s| format!("{}/{}", s.ecosystem, s.package))
                        .collect::<Vec<_>>()
                        .join(", "),
                    t.last_used_at
                        .map(|at| at.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "never".to_owned()),
                    state,
                ]);
            }
            println!("{table}");
        }
        TokenAction::Revoke { id } => {
            client.delete(&format!("/api/v1/tokens/{id}")).await?;
            println!("revoked {id}");
        }
    }
    Ok(())
}

async fn version(client: &Client, action: VersionAction) -> anyhow::Result<()> {
    let (package, version, path, body) = match action {
        VersionAction::Yank {
            package,
            version,
            reason,
        } => {
            let path = format!("/cli/v1/packages/{package}/versions/{version}/yank");
            (package, version, path, Some(Yank { reason }))
        }
        VersionAction::Unyank { package, version } => {
            let path = format!("/cli/v1/packages/{package}/versions/{version}/unyank");
            (package, version, path, None)
        }
    };

    let result: Version = match &body {
        Some(body) => client.send(Method::POST, &path, body).await?,
        None => client.post_empty(&path).await?,
    };
    let state = if result.yanked.is_some() {
        "yanked"
    } else {
        "unyanked"
    };
    println!("{state} {package}@{version}");
    Ok(())
}

async fn audit(client: &Client, package: Option<String>, limit: i64) -> anyhow::Result<()> {
    let mut path = format!("/api/v1/audit?limit={limit}");
    if let Some(package) = package {
        path.push_str(&format!("&package={}", urlencode(&package)));
    }
    let entries: Vec<AuditEntry> = client.get(&path).await?;

    let mut table = Table::new();
    table
        .load_preset(UTF8_BORDERS_ONLY)
        .set_header(["at", "actor", "action", "target"]);
    for e in entries {
        let target = [e.package, e.version]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("@");
        table.add_row([
            e.at.format("%Y-%m-%d %H:%M").to_string(),
            e.actor,
            e.action,
            target,
        ]);
    }
    println!("{table}");
    Ok(())
}
