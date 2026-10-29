mod client;
mod commands;
mod dispatch;
mod manifest;
mod package_auth;
mod session;
mod store;
mod trust;

use std::{ffi::OsString, path::PathBuf};

use clap::{Parser, Subcommand};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use registry_api::{Target, WhoAmI};

use commands::{init, packages, publish, registry::RegistryCommand};
use session::Settings;
use store::Store;

#[derive(Parser)]
#[command(
    name = "ortzi",
    version = env!("ORTZI_VERSION"),
    about = "ortzi platform CLI",
    long_about = "ortzi platform CLI. Install packages from the registry and run them as \
                  `ortzi <package> ...`.",
    allow_external_subcommands = true
)]
struct Cli {
    #[command(flatten)]
    settings: Settings,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Install packages, e.g. `ortzi install ff` or `ortzi install ff@1.2.0`
    Install {
        #[arg(required = true)]
        packages: Vec<String>,
        #[arg(long, help = "Reinstall even if already installed")]
        force: bool,
    },
    /// Remove installed packages
    Uninstall {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Update ortzi and installed packages to their latest versions
    Update { packages: Vec<String> },
    /// Update ortzi itself from the registry
    SelfUpdate {
        #[arg(long, help = "Install this exact version, even if older")]
        version: Option<String>,
    },
    /// List installed packages
    List,
    /// Search the registry
    Search { query: Option<String> },
    /// Show a package's versions
    Info { package: String },
    /// Log in to the registry, or to an installed package's service
    Login { package: Option<String> },
    /// Forget the registry session, or a package's
    Logout { package: Option<String> },
    /// Show who the registry thinks you are
    Whoami,
    /// Build and publish the package described by ortzi.yaml
    Publish {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long = "target", help = "Only build and upload these targets")]
        targets: Vec<Target>,
        #[arg(long, help = "Build only; don't upload")]
        dry_run: bool,
        #[arg(
            long,
            help = "Upload without publishing, to add targets from another machine"
        )]
        no_finalize: bool,
        #[arg(
            long,
            help = "Fail instead of skipping when the version is already published"
        )]
        fail_if_exists: bool,
    },
    /// Create ortzi.yaml and a Forgejo publish workflow for a package
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        no_workflow: bool,
    },
    /// Registry administration
    Registry {
        #[command(subcommand)]
        command: RegistryCommand,
    },
    /// Print the directory installed binaries are linked into, for PATH
    BinDir,
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let settings = cli.settings;

    let Some(command) = cli.command else {
        packages::list()?;
        println!("\nrun `ortzi --help` for commands, `ortzi search` for packages");
        return Ok(());
    };

    match command {
        Command::Install { packages, force } => {
            packages::install(&settings, &packages, force).await
        }
        Command::Uninstall { packages } => packages::uninstall(&packages),
        Command::Update { packages } => packages::update(&settings, &packages).await,
        Command::SelfUpdate { version } => {
            packages::self_update(&settings, version.as_deref()).await
        }
        Command::List => packages::list(),
        Command::Search { query } => packages::search(&settings, query.as_deref()).await,
        Command::Info { package } => packages::info(&settings, &package).await,
        Command::Login { package: None } => cli_auth::commands::login(&settings.oauth()).await,
        Command::Login {
            package: Some(package),
        } => {
            let (name, auth) = package_auth::installed(&package)?;
            cli_auth::commands::login(&package_auth::oauth(&name, &auth)).await
        }
        Command::Logout { package: None } => cli_auth::commands::logout(&settings.oauth()),
        Command::Logout {
            package: Some(package),
        } => {
            let (name, auth) = package_auth::installed(&package)?;
            cli_auth::commands::logout(&package_auth::oauth(&name, &auth))
        }
        Command::Whoami => whoami(&settings).await,
        Command::Publish {
            path,
            targets,
            dry_run,
            no_finalize,
            fail_if_exists,
        } => {
            publish::publish(
                &settings,
                publish::Options {
                    path: &path,
                    targets: &targets,
                    dry_run,
                    no_finalize,
                    fail_if_exists,
                },
            )
            .await
        }
        Command::Init {
            path,
            name,
            force,
            no_workflow,
        } => init::init(init::Options {
            path: &path,
            name: name.as_deref(),
            force,
            no_workflow,
        }),
        Command::Registry { command } => commands::registry::run(&settings, command).await,
        Command::BinDir => {
            println!("{}", Store::open()?.bin_dir().display());
            Ok(())
        }
        Command::External(args) => dispatch::exec(&settings, args).await,
    }
}

async fn whoami(settings: &Settings) -> anyhow::Result<()> {
    let me: WhoAmI = settings.reader().await?.get("/api/v1/whoami").await?;
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.add_row(["subject", &me.subject]);
    table.add_row(["kind", &me.kind]);
    table.add_row(["admin", if me.admin { "yes" } else { "no" }]);
    table.add_row(["registry", &settings.registry]);
    if !me.scopes.is_empty() {
        let scopes: Vec<String> = me
            .scopes
            .iter()
            .map(|s| format!("{}/{}", s.ecosystem, s.package))
            .collect();
        table.add_row(["scopes", &scopes.join(", ")]);
    }
    println!("{table}");
    Ok(())
}
