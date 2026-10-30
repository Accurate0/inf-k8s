mod grafana;
mod logs;
mod metrics;

use clap::{Args, CommandFactory, Parser, Subcommand};
use cli_auth::{App, OAuthClient, TokenSource};
use grafana::Grafana;

const APP: App = App {
    name: "obs",
    scopes: "openid email profile",
    login_hint: "run `ortzi login obs` or set OBS_TOKEN",
};

#[derive(Args, Debug)]
struct Global {
    #[arg(
        long,
        global = true,
        env = "OBS_GRAFANA_URL",
        default_value = "https://grafana.ortzi.cloud"
    )]
    grafana: String,

    #[arg(
        long,
        global = true,
        env = "OBS_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/grafana-cli",
        hide_short_help = true
    )]
    issuer: String,

    #[arg(
        long,
        global = true,
        env = "OBS_OIDC_CLIENT_ID",
        default_value = "grafana-cli",
        hide_short_help = true
    )]
    client_id: String,

    #[arg(
        long,
        global = true,
        env = "OBS_TOKEN",
        hide_env_values = true,
        help = "Use this token instead of the stored login"
    )]
    token: Option<String>,
}

#[derive(Parser)]
#[command(name = "obs", about = "Query logs and metrics through Grafana")]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Query Loki
    Logs(logs::LogsArgs),
    /// Query Prometheus
    Metrics(metrics::MetricsArgs),
    /// Log in through the browser and store the session
    Login,
    /// Forget the stored session
    Logout,
    /// Show the Grafana user the token maps to
    Whoami,
}

#[derive(Parser)]
#[command(name = "logs", about = "Query Loki through Grafana")]
struct LogsCli {
    #[command(flatten)]
    global: Global,
    #[command(flatten)]
    args: logs::LogsArgs,
}

#[derive(Parser)]
#[command(name = "metrics", about = "Query Prometheus through Grafana")]
struct MetricsCli {
    #[command(flatten)]
    global: Global,
    #[command(flatten)]
    args: metrics::MetricsArgs,
}

fn invoked_as() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg| {
            std::path::Path::new(&arg)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

fn parse(name: &str) -> (Global, Command) {
    match name {
        "logs" => {
            let cli = LogsCli::parse();
            (cli.global, Command::Logs(cli.args))
        }
        "metrics" => {
            let cli = MetricsCli::parse();
            (cli.global, Command::Metrics(cli.args))
        }
        _ => {
            let cli = Cli::parse();
            (cli.global, cli.command)
        }
    }
}

async fn grafana(global: &Global, oauth: OAuthClient) -> anyhow::Result<Grafana> {
    let token = match global.token.clone().filter(|t| !t.is_empty()) {
        Some(token) => token,
        None => TokenSource::acquire(oauth).await?.access_token(),
    };
    Grafana::new(&global.grafana, token)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let name = invoked_as();
    clap_complete::CompleteEnv::with_factory(move || match name.as_str() {
        "logs" => LogsCli::command(),
        "metrics" => MetricsCli::command(),
        _ => Cli::command(),
    })
    .complete();
    let (global, command) = parse(&invoked_as());
    let oauth = OAuthClient::new(APP, &global.issuer, &global.client_id);

    match command {
        Command::Login => cli_auth::commands::login(&oauth).await,
        Command::Logout => cli_auth::commands::logout(&oauth),
        Command::Whoami => {
            #[derive(serde::Deserialize)]
            struct User {
                login: String,
                #[serde(rename = "isGrafanaAdmin", default)]
                admin: bool,
            }
            let user: User = grafana(&global, oauth).await?.get("/api/user", &[]).await?;
            println!("grafana  {}", global.grafana);
            println!("user     {}", user.login);
            println!("admin    {}", if user.admin { "yes" } else { "no" });
            Ok(())
        }
        Command::Logs(args) => logs::run(&grafana(&global, oauth).await?, args).await,
        Command::Metrics(args) => metrics::run(&grafana(&global, oauth).await?, args).await,
    }
}
