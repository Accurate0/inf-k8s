use clap::Parser;

/// Runtime configuration, sourced entirely from the environment to match the other
/// platform-services. Postgres is required; Dragonfly and OTLP are optional.
#[derive(Debug, Clone, Parser)]
pub struct Config {
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// Dragonfly (redis-protocol) URL. When unset, the L2 cache is disabled.
    #[arg(long, env = "REDIS_URL")]
    pub redis_url: Option<String>,

    /// Address the gRPC server binds to.
    #[arg(long, env = "GRPC_ADDR", default_value = "0.0.0.0:50051")]
    pub grpc_addr: String,

    #[arg(long, env = "OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    #[arg(long, env = "OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    #[arg(long, env = "OIDC_JWKS_URI")]
    pub oidc_jwks_uri: Option<String>,

    #[arg(long, env = "OIDC_USERINFO_ENDPOINT")]
    pub oidc_userinfo_endpoint: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Config::parse()
    }
}
