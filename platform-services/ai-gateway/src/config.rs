/// Gateway-level configuration that isn't provider-specific. Upstream providers live in
/// [`crate::providers::Registry`].
#[derive(Clone, Debug)]
pub struct Config {
    /// Bearer token guarding `/admin/*`. Empty disables the admin routes entirely.
    pub admin_token: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            admin_token: std::env::var("ADMIN_TOKEN").unwrap_or_default(),
        }
    }
}
