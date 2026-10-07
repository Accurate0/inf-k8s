use anyhow::{Context as _, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub issuer: String,
    pub client_id: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl Credentials {
    const EXPIRY_SKEW_SECS: i64 = 60;

    pub fn path() -> anyhow::Result<PathBuf> {
        let dir = dirs::config_dir().context("no config directory for this user")?;

        Ok(dir.join("aig").join("credentials.json"))
    }

    pub fn load(issuer: &str, client_id: &str) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path().ok()?).ok()?;
        let credentials: Self = serde_json::from_str(&text).ok()?;

        (credentials.issuer == issuer && credentials.client_id == client_id).then_some(credentials)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let path = Self::path()?;
        let dir = path.parent().context("credentials path has no parent")?;
        std::fs::create_dir_all(dir)?;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;

        Ok(())
    }

    pub fn delete() -> anyhow::Result<bool> {
        let path = Self::path()?;

        if !path.exists() {
            return Ok(false);
        }

        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;

        Ok(true)
    }

    pub fn is_fresh(&self) -> bool {
        self.expires_at - Self::EXPIRY_SKEW_SECS > chrono::Utc::now().timestamp()
    }

    pub fn display_name(&self) -> &str {
        self.username.as_deref().unwrap_or("unknown")
    }
}

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct IdClaims {
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    sub: Option<String>,
}

struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn new() -> Self {
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

        Self {
            verifier,
            challenge,
        }
    }
}

struct Callback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    port: u16,
}

impl Callback {
    const PATH: &'static str = "/callback";
    const PAGE: &'static str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>aig</title></head><body style=\"font-family: sans-serif; margin: 4rem\"><h2>aig</h2><p>MESSAGE</p></body></html>";

    async fn bind() -> anyhow::Result<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("binding loopback callback listener")?;
        let port = v4.local_addr()?.port();
        let v6 = TcpListener::bind(("::1", port)).await.ok();

        Ok(Self { v4, v6, port })
    }

    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}{}", self.port, Self::PATH)
    }

    async fn accept(&self) -> std::io::Result<TcpStream> {
        let Some(v6) = &self.v6 else {
            return self.v4.accept().await.map(|(stream, _)| stream);
        };

        tokio::select! {
            accepted = self.v4.accept() => accepted.map(|(stream, _)| stream),
            accepted = v6.accept() => accepted.map(|(stream, _)| stream),
        }
    }

    async fn wait_for_code(&self, state: &str) -> anyhow::Result<String> {
        loop {
            let mut stream = self.accept().await?;
            let mut buffer = vec![0u8; 8192];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..read]).into_owned();

            let target = head.split_whitespace().nth(1).unwrap_or_default();
            let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
                Self::respond(&mut stream, 400, "Bad request.").await;
                continue;
            };

            if url.path() != Self::PATH {
                Self::respond(&mut stream, 404, "Not found.").await;
                continue;
            }

            let param = |name: &str| {
                url.query_pairs()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.into_owned())
            };

            if let Some(error) = param("error") {
                let description = param("error_description").unwrap_or_default();
                Self::respond(&mut stream, 400, "Login failed. Return to the terminal.").await;
                bail!("authorization failed: {error} {description}");
            }

            if param("state").as_deref() != Some(state) {
                Self::respond(&mut stream, 400, "State mismatch.").await;
                bail!("authorization response state did not match");
            }

            let Some(code) = param("code") else {
                Self::respond(&mut stream, 400, "Missing authorization code.").await;
                bail!("authorization response had no code");
            };

            Self::respond(&mut stream, 200, "Logged in. You can close this tab.").await;

            return Ok(code);
        }
    }

    async fn respond(stream: &mut TcpStream, status: u16, message: &str) {
        let body = Self::PAGE.replace("MESSAGE", message);
        let response = format!(
            "HTTP/1.1 {status} OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );

        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}

#[derive(Clone)]
pub struct OAuthClient {
    http: reqwest::Client,
    issuer: String,
    client_id: String,
}

impl OAuthClient {
    const SCOPES: &'static str = "openid email profile groups";
    const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
    const DEFAULT_EXPIRY_SECS: i64 = 300;

    pub fn new(issuer: &str, client_id: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("aig")
                .build()
                .unwrap_or_default(),
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id: client_id.to_owned(),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    async fn discover(&self) -> anyhow::Result<Discovery> {
        let url = format!("{}/.well-known/openid-configuration", self.issuer);

        self.http
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("fetching {url}"))?
            .json()
            .await
            .context("parsing openid configuration")
    }

    pub async fn login(&self) -> anyhow::Result<Credentials> {
        let discovery = self.discover().await?;
        let callback = Callback::bind().await?;
        let redirect_uri = callback.redirect_uri();
        let pkce = Pkce::new();
        let state = uuid::Uuid::new_v4().simple().to_string();

        let mut authorize = reqwest::Url::parse(&discovery.authorization_endpoint)
            .context("invalid authorization endpoint")?;
        authorize
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", Self::SCOPES)
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256");

        eprintln!("Opening the browser to log in. If it does not open, visit:\n\n  {authorize}\n");

        if let Err(e) = open::that_detached(authorize.as_str()) {
            eprintln!("could not open a browser: {e}");
        }

        let code = tokio::time::timeout(Self::LOGIN_TIMEOUT, callback.wait_for_code(&state))
            .await
            .context("timed out waiting for the login to complete")??;

        let credentials = self
            .token(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect_uri),
                ("client_id", &self.client_id),
                ("code_verifier", &pkce.verifier),
            ])
            .await?;

        credentials.save()?;

        Ok(credentials)
    }

    pub async fn refresh(&self, current: &Credentials) -> anyhow::Result<Credentials> {
        let refresh_token = current
            .refresh_token
            .as_deref()
            .context("no refresh token stored")?;

        let mut credentials = self
            .token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &self.client_id),
            ])
            .await?;

        if credentials.refresh_token.is_none() {
            credentials.refresh_token = current.refresh_token.clone();
        }

        if credentials.username.is_none() {
            credentials.username = current.username.clone();
        }

        credentials.save()?;

        Ok(credentials)
    }

    async fn token(&self, form: &[(&str, &str)]) -> anyhow::Result<Credentials> {
        let discovery = self.discover().await?;

        let response = self
            .http
            .post(&discovery.token_endpoint)
            .form(form)
            .send()
            .await
            .context("calling the token endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("token endpoint returned {status}: {body}");
        }

        let token: TokenResponse = response.json().await.context("parsing token response")?;
        let expires_in = token.expires_in.unwrap_or(Self::DEFAULT_EXPIRY_SECS);

        Ok(Credentials {
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            username: token.id_token.as_deref().and_then(Self::username_of),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: chrono::Utc::now().timestamp() + expires_in,
        })
    }

    fn username_of(id_token: &str) -> Option<String> {
        let payload = id_token.split('.').nth(1)?;
        let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
        let claims: IdClaims = serde_json::from_slice(&bytes).ok()?;

        claims.preferred_username.or(claims.email).or(claims.sub)
    }
}

pub struct TokenSource;

impl TokenSource {
    pub async fn acquire(oauth: &OAuthClient) -> anyhow::Result<Credentials> {
        let stored = Credentials::load(oauth.issuer(), oauth.client_id());

        match stored {
            Some(credentials) if credentials.is_fresh() => Ok(credentials),
            Some(credentials) => match oauth.refresh(&credentials).await {
                Ok(refreshed) => Ok(refreshed),
                Err(e) => Self::interactive_login(oauth, &format!("session expired ({e})")).await,
            },
            None => Self::interactive_login(oauth, "not logged in").await,
        }
    }

    async fn interactive_login(oauth: &OAuthClient, reason: &str) -> anyhow::Result<Credentials> {
        if !std::io::stdin().is_terminal() {
            bail!("{reason}; run `aig login` or set AIG_ADMIN_TOKEN");
        }

        eprintln!("{reason}");
        oauth.login().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let pkce = Pkce::new();

        assert_eq!(pkce.verifier.len(), 64);
        assert_eq!(
            pkce.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
        assert_ne!(pkce.verifier, Pkce::new().verifier);
    }

    #[test]
    fn username_is_read_from_the_id_token_payload() {
        let payload = URL_SAFE_NO_PAD
            .encode(r#"{"sub":"abc","preferred_username":"anurag","email":"a@b.c"}"#);
        let token = format!("header.{payload}.signature");

        assert_eq!(OAuthClient::username_of(&token).as_deref(), Some("anurag"));
        assert_eq!(OAuthClient::username_of("garbage"), None);
    }

    #[test]
    fn freshness_accounts_for_skew() {
        let mut credentials = Credentials {
            issuer: "i".to_owned(),
            client_id: "c".to_owned(),
            access_token: "t".to_owned(),
            refresh_token: None,
            expires_at: chrono::Utc::now().timestamp() + 30,
            username: None,
        };
        assert!(!credentials.is_fresh());

        credentials.expires_at += 600;
        assert!(credentials.is_fresh());
    }

    #[tokio::test]
    async fn callback_returns_the_code_for_a_matching_state() {
        let callback = Callback::bind().await.unwrap();
        let port = callback.port;

        let browser = tokio::spawn(async move {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            stream
                .write_all(
                    b"GET /callback?code=abc123&state=xyz HTTP/1.1\r\nhost: localhost\r\n\r\n",
                )
                .await
                .unwrap();

            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        });

        let code = callback.wait_for_code("xyz").await.unwrap();
        assert_eq!(code, "abc123");
        assert!(browser.await.unwrap().starts_with("HTTP/1.1 200"));
    }

    #[tokio::test]
    async fn callback_rejects_a_state_mismatch() {
        let callback = Callback::bind().await.unwrap();
        let port = callback.port;

        tokio::spawn(async move {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let _ = stream
                .write_all(b"GET /callback?code=abc123&state=evil HTTP/1.1\r\n\r\n")
                .await;
        });

        assert!(callback.wait_for_code("xyz").await.is_err());
    }
}
