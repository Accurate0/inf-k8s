use std::path::PathBuf;

use anyhow::Context;
use clap::Subcommand;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use cynic::{MutationBuilder, QueryBuilder};

use super::{artifact, harbor};
use crate::{
    client::{Client, registry_id},
    graphql as gql,
    session::Settings,
};

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
    /// Mint a short-lived Harbor robot into the docker config, e.g. `containers:pull charts:push`
    Login {
        #[arg(required = true, value_name = "PROJECT:ACTIONS")]
        specs: Vec<String>,
    },
    /// Revoke the Harbor robot in the docker config and remove it
    Logout {
        #[arg(long, default_value = "registry.anurag.sh")]
        registry: String,
    },
    /// Push a directory as a single-layer OCI artifact, e.g. rendered manifests for Flux
    Push {
        dir: PathBuf,
        #[arg(value_name = "oci://HOST/PROJECT/NAME")]
        reference: String,
        #[arg(long = "tag", required = true)]
        tags: Vec<String>,
        #[arg(long, value_name = "SHA")]
        revision: Option<String>,
        /// Cosign-sign the pushed digest with the registry key
        #[arg(long)]
        sign: bool,
    },
    /// Cosign-sign an image in Harbor with the registry key
    Sign {
        #[arg(value_name = "IMAGE@DIGEST")]
        image: String,
    },
    /// Show the audit log
    Audit {
        #[arg(long)]
        package: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i32,
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
    /// Hide a package from search; installs and updates keep working, new versions are refused
    Archive { name: String },
    /// Restore an archived package
    Unarchive { name: String },
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
        expires_in_days: Option<i32>,
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
        RegistryCommand::Login { specs } => harbor::login(&client, &specs).await.map(|_| ()),
        RegistryCommand::Logout { registry } => harbor::logout(&client, &registry).await,
        RegistryCommand::Sign { image } => harbor::sign(&client, &image).await,
        RegistryCommand::Push {
            dir,
            reference,
            tags,
            revision,
            sign,
        } => artifact::push(&client, &dir, &reference, &tags, revision.as_deref(), sign).await,
    }
}

fn publisher_row(publisher: gql::TrustedPublisher) -> [String; 5] {
    [
        publisher.id.into_inner(),
        format!(
            "{}/{}",
            publisher.ecosystem,
            publisher.package.map(|p| p.name).unwrap_or_default()
        ),
        publisher.repository,
        publisher.ref_pattern.unwrap_or_else(|| "*".to_owned()),
        publisher.workflow.unwrap_or_else(|| "*".to_owned()),
    ]
}

async fn add_publisher(client: &Client, package: &str, trust: TrustArgs) -> anyhow::Result<()> {
    let Some(repository) = trust.repository else {
        anyhow::bail!("--forgejo-repo is required");
    };
    let publisher = client
        .graph()
        .run(gql::CreateTrustedPublisher::build(
            gql::CreateTrustedPublisherVariables {
                input: gql::CreateTrustedPublisherInput {
                    package: package.to_owned(),
                    repository,
                    ref_pattern: trust.ref_pattern,
                    workflow: trust.workflow,
                },
            },
        ))
        .await?
        .registry_create_trusted_publisher;
    println!(
        "trusted {} ({}{}) to publish {package} [{}]",
        publisher.repository,
        publisher.ref_pattern.as_deref().unwrap_or("any ref"),
        publisher
            .workflow
            .map(|w| format!(", {w}"))
            .unwrap_or_default(),
        publisher.id.inner()
    );
    Ok(())
}

async fn package(client: &Client, action: PackageAction) -> anyhow::Result<()> {
    let graph = client.graph();
    let named = |name: &str| gql::NameVariables {
        name: name.to_owned(),
    };

    match action {
        PackageAction::Create {
            name,
            description,
            binary,
            aliases,
            trust,
        } => {
            let package = graph
                .run(gql::CreatePackage::build(gql::CreatePackageVariables {
                    input: gql::CreatePackageInput {
                        name: name.clone(),
                        description,
                        binary_name: binary,
                        aliases,
                    },
                }))
                .await?
                .registry_create_package;
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
            let package = graph
                .run(gql::UpdatePackage::build(gql::UpdatePackageVariables {
                    name,
                    input: gql::UpdatePackageInput {
                        description,
                        binary_name: binary,
                        aliases,
                    },
                }))
                .await?
                .registry_update_package;
            println!("updated {}", package.name);
        }
        PackageAction::Archive { name } => {
            let package = graph
                .run(gql::ArchivePackage::build(named(&name)))
                .await?
                .registry_archive_package;
            println!("archived {}", package.name);
        }
        PackageAction::Unarchive { name } => {
            let package = graph
                .run(gql::UnarchivePackage::build(named(&name)))
                .await?
                .registry_unarchive_package;
            println!("unarchived {}", package.name);
        }
        PackageAction::Delete { name } => {
            let deleted = graph
                .run(gql::DeletePackage::build(named(&name)))
                .await?
                .registry_delete_package;
            println!("deleted {name} [{}]", deleted.inner());
        }
    }
    Ok(())
}

async fn publisher(client: &Client, action: PublisherAction) -> anyhow::Result<()> {
    match action {
        PublisherAction::Add { package, trust } => add_publisher(client, &package, trust).await?,
        PublisherAction::List { package } => {
            let publishers = client
                .graph()
                .run(gql::TrustedPublishersQuery::build(
                    gql::PackageFilterVariables { package },
                ))
                .await?
                .registry_trusted_publishers
                .context("the graph returned no registryTrustedPublishers")?;
            let mut table = Table::new();
            table.load_preset(UTF8_BORDERS_ONLY).set_header([
                "id",
                "package",
                "repository",
                "ref",
                "workflow",
            ]);
            for p in publishers {
                table.add_row(publisher_row(p));
            }
            println!("{table}");
        }
        PublisherAction::Rm { id } => {
            let removed = client
                .graph()
                .run(gql::DeleteTrustedPublisher::build(gql::IdVariables {
                    id: registry_id("trustedPublisher", &id),
                }))
                .await?
                .registry_delete_trusted_publisher;
            println!("removed {}", removed.inner());
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
            let created = client
                .graph()
                .run(gql::CreatePublishToken::build(
                    gql::CreatePublishTokenVariables {
                        input: gql::CreatePublishTokenInput {
                            name: name.unwrap_or_else(|| packages.join("+")),
                            packages,
                            expires_in_days,
                        },
                    },
                ))
                .await?
                .registry_create_publish_token;
            eprintln!(
                "token {} (shown once; install.sh reads it as ORTZI_TOKEN):",
                created.token.id.inner()
            );
            println!("{}", created.secret);
        }
        TokenAction::List => {
            let tokens = client
                .graph()
                .run(gql::PublishTokensQuery::build(()))
                .await?
                .registry_publish_tokens
                .context("the graph returned no registryPublishTokens")?;
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
                    t.id.into_inner(),
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
            let revoked = client
                .graph()
                .run(gql::RevokePublishToken::build(gql::IdVariables {
                    id: registry_id("publishToken", &id),
                }))
                .await?
                .registry_revoke_publish_token;
            println!("revoked {}", revoked.name);
        }
    }
    Ok(())
}

async fn version(client: &Client, action: VersionAction) -> anyhow::Result<()> {
    let graph = client.graph();
    let id = |package: &str, version: &str| registry_id("version", &format!("{package}@{version}"));

    let (package, version, result) = match action {
        VersionAction::Yank {
            package,
            version,
            reason,
        } => {
            let result = graph
                .run(gql::YankVersion::build(gql::YankVariables {
                    id: id(&package, &version),
                    reason,
                }))
                .await?
                .registry_yank_version;
            (package, version, result)
        }
        VersionAction::Unyank { package, version } => {
            let result = graph
                .run(gql::UnyankVersion::build(gql::IdVariables {
                    id: id(&package, &version),
                }))
                .await?
                .registry_unyank_version;
            (package, version, result)
        }
    };

    match result.yanked {
        Some(yanked) => println!(
            "yanked {package}@{version}{}",
            yanked
                .reason
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default()
        ),
        None => println!("unyanked {package}@{version}"),
    }
    Ok(())
}

async fn audit(client: &Client, package: Option<String>, limit: i32) -> anyhow::Result<()> {
    let entries = client
        .graph()
        .run(gql::AuditQuery::build(gql::AuditVariables {
            package,
            limit,
        }))
        .await?
        .registry_audit
        .context("the graph returned no registryAudit")?;

    let mut table = Table::new();
    table
        .load_preset(UTF8_BORDERS_ONLY)
        .set_header(["at", "actor", "action", "target"]);
    for e in entries {
        let target = [e.package_name, e.version]
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
