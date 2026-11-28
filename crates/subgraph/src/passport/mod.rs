mod mint;

use serde::{Deserialize, Serialize};

pub use mint::Minter;

use crate::access::Identity;

pub const HEADER: &str = "x-graph-passport";
pub const ISSUER: &str = "graph-router";
pub const JWKS_PATH: &str = "/.well-known/passport.jwks";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Passport {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub identity: Identity,
}
