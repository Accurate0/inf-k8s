use crate::ObjectRegistryJwtClaims;
use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, jwk};
use reqwest::{Client, Method, Url, header::CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::any::TypeId;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiClientError {
    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("JWK not found for kid: {0}")]
    JwkNotFound(String),
    #[error("Other: {0}")]
    Other(String),
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    client: Client,
    private_key_pem: Vec<u8>,
    kid: String,
    issuer: String,
}

impl ApiClient {
    /// Construct a new `ApiClient` using the fixed base URL and the provided credentials.
    /// The API base URL is fixed to `https://object-registry.inf-k8s.net/v1` and cannot be changed.
    pub fn new(
        private_key_pem: impl Into<Vec<u8>>,
        kid: impl Into<String>,
        issuer: impl Into<String>,
    ) -> Self {
        let base_url = "https://object-registry.inf-k8s.net/v1".to_string();
        let client = Client::builder()
            .build()
            .expect("failed to build reqwest client");
        Self {
            base_url,
            client,
            private_key_pem: private_key_pem.into(),
            kid: kid.into(),
            issuer: issuer.into(),
        }
    }

    fn make_encoding_key(&self) -> Result<EncodingKey, jsonwebtoken::errors::Error> {
        EncodingKey::from_rsa_pem(&self.private_key_pem)
    }

    fn make_header(&self) -> Header {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());
        header
    }

    fn make_claims(&self, audience: &str) -> ObjectRegistryJwtClaims {
        let now = chrono::offset::Utc::now().timestamp();
        ObjectRegistryJwtClaims {
            iat: now,
            exp: now + 900, // 15 minutes
            aud: audience.to_string(),
            iss: self.issuer.clone(),
            sub: self.issuer.clone(),
            role: vec![],
        }
    }

    pub fn generate_jwt(&self, audience: &str) -> Result<String, ApiClientError> {
        let key = self.make_encoding_key()?;
        let header = self.make_header();
        let claims = self.make_claims(audience);
        let token = jsonwebtoken::encode(&header, &claims, &key)?;
        Ok(token)
    }

    pub async fn validate_token(&self, token: &str) -> Result<bool, ApiClientError> {
        let jwks_url = format!(
            "{}/.well-known/jwks.json",
            self.base_url.trim_end_matches('/')
        );
        let resp = self.client.get(jwks_url).send().await?.error_for_status()?;
        let jwks: JwkSet = resp.json().await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(std::slice::from_ref(&self.issuer));
        validation.set_issuer(&["object-registry"]);

        let decoding_key = DecodingKey::from_jwk(
            jwks.keys
                .first()
                .ok_or_else(|| ApiClientError::JwkNotFound("could not find jwk".to_string()))?,
        )?;

        decode::<ObjectRegistryJwtClaims>(token, &decoding_key, &validation)?;

        Ok(true)
    }

    /// Centralized request builder that formats the fixed base URL with the provided resource.
    fn get_default_request(&self, resource: &str, method: Method) -> reqwest::RequestBuilder {
        let base = self.base_url.trim_end_matches('/');
        let resource = resource.trim_start_matches('/');
        let url = format!("{}/{}", base, resource);
        self.client
            .request(method, url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
    }

    pub async fn put_object(
        &self,
        namespace: &str,
        object: &str,
        version: Option<&str>,
        body: &[u8],
    ) -> Result<(), ApiClientError> {
        let rel = format!("{}/{}", namespace, object);
        let base = self.base_url.trim_end_matches('/');
        let resource = rel.trim_start_matches('/');
        let mut url = Url::parse(&format!("{}/{}", base, resource))
            .map_err(|e| ApiClientError::Other(e.to_string()))?;
        if let Some(v) = version {
            url.query_pairs_mut().append_pair("version", v);
        }
        let jwt = self.generate_jwt("object-registry")?;
        let _resp = self
            .client
            .put(url)
            .bearer_auth(jwt)
            .body(body.to_vec())
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Generic get_object: deserialize response into `T` based on Content-Type (json/yaml).
    /// If `T` is `String`, return base64-encoded payload.
    pub async fn get_object<T>(
        &self,
        namespace: &str,
        object: &str,
        version: Option<&str>,
    ) -> Result<T, ApiClientError>
    where
        T: DeserializeOwned + 'static,
    {
        let rel = format!("{}/{}", namespace, object);
        let base = self.base_url.trim_end_matches('/');
        let resource = rel.trim_start_matches('/');
        let mut url = Url::parse(&format!("{}/{}", base, resource))
            .map_err(|e| ApiClientError::Other(e.to_string()))?;
        if let Some(v) = version {
            url.query_pairs_mut().append_pair("version", v);
        }
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .client
            .request(Method::GET, url)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let bytes = resp.bytes().await?;
        let bytes_vec = bytes.to_vec();

        // build request and attach optional version query param
        // JSON path: expect { key: String, payload: ... }
        if content_type.contains("json") {
            let root: serde_json::Value = serde_json::from_slice(&bytes_vec)?;
            let payload = root
                .get("payload")
                .ok_or_else(|| ApiClientError::Other("missing payload field".to_string()))?
                .clone();

            if payload.is_string() {
                // payload is a string
                if TypeId::of::<T>() == TypeId::of::<String>() {
                    let s = payload.as_str().unwrap().to_string();
                    // return as T
                    let t = serde_json::from_value::<T>(serde_json::Value::String(s))?;
                    return Ok(t);
                } else {
                    return Err(ApiClientError::Other(
                        "payload is a raw string; cannot deserialize into requested type"
                            .to_string(),
                    ));
                }
            }

            // payload is structured JSON -> deserialize into T
            let t = serde_json::from_value::<T>(payload)?;
            return Ok(t);
        }

        // YAML path: expect { key: String, payload: ... }
        if content_type.contains("yaml") || content_type.contains("yml") {
            let root: serde_yaml::Value = serde_yaml::from_slice(&bytes_vec)?;
            let payload = root
                .get("payload")
                .ok_or_else(|| ApiClientError::Other("missing payload field".to_string()))?
                .clone();

            // Always attempt to deserialize payload into T, regardless of whether it's a YAML string.
            let t = serde_yaml::from_value::<T>(payload)?;
            return Ok(t);
        }

        // Fallback: non-json/yaml payloads — if caller wants String, return base64 of raw bytes
        if TypeId::of::<T>() == TypeId::of::<String>() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes_vec);
            let json = serde_json::to_string(&b64)?;
            let t = serde_json::from_str::<T>(&json)?;
            return Ok(t);
        }

        // final attempt: try to deserialize raw body as JSON or YAML into T
        if let Ok(parsed) = serde_json::from_slice::<T>(&bytes_vec) {
            return Ok(parsed);
        }
        if let Ok(parsed) = serde_yaml::from_slice::<T>(&bytes_vec) {
            return Ok(parsed);
        }

        Err(ApiClientError::Other(
            "unable to deserialize object to requested type".to_string(),
        ))
    }

    pub async fn post_event(
        &self,
        namespace: &str,
        req: &crate::types::EventRequest,
    ) -> Result<crate::types::CreatedResponse, ApiClientError> {
        let rel = format!("events/{}", namespace);
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .get_default_request(&rel, Method::POST)
            .bearer_auth(jwt)
            .json(req)
            .send()
            .await?
            .error_for_status()?;
        let created: crate::types::CreatedResponse = resp.json().await?;
        Ok(created)
    }

    pub async fn put_event(
        &self,
        namespace: &str,
        id: &str,
        req: &crate::types::EventRequest,
    ) -> Result<crate::types::CreatedResponse, ApiClientError> {
        let rel = format!("events/{}/{}", namespace, id);
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .get_default_request(&rel, Method::PUT)
            .bearer_auth(jwt)
            .json(req)
            .send()
            .await?
            .error_for_status()?;
        let created: crate::types::CreatedResponse = resp.json().await?;
        Ok(created)
    }

    pub async fn delete_event(&self, namespace: &str, id: &str) -> Result<(), ApiClientError> {
        let rel = format!("events/{}/{}", namespace, id);
        let jwt = self.generate_jwt("object-registry")?;
        let _resp = self
            .get_default_request(&rel, Method::DELETE)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_events(
        &self,
        namespace: &str,
    ) -> Result<Vec<crate::types::EventResponse>, ApiClientError> {
        let rel = format!("events/{}", namespace);
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .get_default_request(&rel, Method::GET)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;
        let arr: Vec<crate::types::EventResponse> = resp.json().await?;
        Ok(arr)
    }

    /// Perform a GET to the API path (appended to base_url) with a freshly-signed JWT.
    pub async fn get(&self, path: &str) -> Result<reqwest::Response, ApiClientError> {
        let rel = path.trim_start_matches('/');
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .get_default_request(rel, Method::GET)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp)
    }

    /// POST JSON body to API path with a freshly-signed JWT.
    pub async fn post_json<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<reqwest::Response, ApiClientError> {
        let rel = path.trim_start_matches('/');
        let jwt = self.generate_jwt("object-registry")?;
        let resp = self
            .get_default_request(rel, Method::POST)
            .bearer_auth(jwt)
            .json(body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp)
    }
}
