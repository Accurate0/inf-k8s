use crate::{JwtValidationError, ObjectRegistryJwtClaims, Role};
use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode};
use reqwest::{Client, Method, Url, header::CONTENT_TYPE};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::any::TypeId;
use std::future::Future;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiClientError {
    #[error("JWT error: {0}")]
    JwtValidation(#[from] JwtValidationError),
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
    pub(crate) base_url: String,
    client: Client,
    private_key_pem: Vec<u8>,
    kid: String,
    issuer: String,
}

impl ApiClient {
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

    pub fn generate_jwt(&self) -> Result<String, ApiClientError> {
        let now = chrono::offset::Utc::now().timestamp();
        let claims = ObjectRegistryJwtClaims {
            iat: now,
            exp: now + 900,
            aud: "object-registry".to_owned(),
            iss: self.issuer.to_owned(),
            sub: "object-registry".to_owned(),
            role: vec![Role::Service],
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());

        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(&self.private_key_pem)?,
        )?;

        Ok(token)
    }

    pub async fn get_jwks(base_url: String) -> Result<JwkSet, ApiClientError> {
        let jwks_url = format!("{}/.well-known/jwks", base_url.trim_end_matches('/'));
        let client = Client::new();
        let resp = client.get(jwks_url).send().await?.error_for_status()?;
        let jwks: JwkSet = resp.json().await?;
        Ok(jwks)
    }

    pub async fn validate_event_token<F, Fut>(
        &self,
        get_jwks_fn: F,
        token: &str,
    ) -> Result<bool, ApiClientError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<JwkSet, ApiClientError>>,
    {
        let jwks = get_jwks_fn(self.base_url.clone()).await?;
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
        public: bool,
        body: &[u8],
    ) -> Result<(), ApiClientError> {
        let rel = if public {
            format!("{}/public/{}", namespace, object)
        } else {
            format!("{}/{}", namespace, object)
        };
        let base = self.base_url.trim_end_matches('/');
        let resource = rel.trim_start_matches('/');
        let mut url = Url::parse(&format!("{}/{}", base, resource))
            .map_err(|e| ApiClientError::Other(e.to_string()))?;
        if let Some(v) = version {
            url.query_pairs_mut().append_pair("version", v);
        }

        let jwt = self.generate_jwt()?;
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

    pub async fn get_object<T>(
        &self,
        namespace: &str,
        object: &str,
        version: Option<&str>,
        public: bool,
    ) -> Result<T, ApiClientError>
    where
        T: DeserializeOwned + 'static,
    {
        let rel = if public {
            format!("{}/public/{}", namespace, object)
        } else {
            format!("{}/{}", namespace, object)
        };
        let base = self.base_url.trim_end_matches('/');
        let resource = rel.trim_start_matches('/');
        let mut url = Url::parse(&format!("{}/{}", base, resource))
            .map_err(|e| ApiClientError::Other(e.to_string()))?;
        if let Some(v) = version {
            url.query_pairs_mut().append_pair("version", v);
        }

        let mut req = self.client.request(Method::GET, url);

        if !public {
            let jwt = self.generate_jwt()?;
            req = req.bearer_auth(jwt);
        }

        let resp = req.send().await?.error_for_status()?;

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let bytes = resp.bytes().await?;
        let bytes_vec = bytes.to_vec();

        if content_type.contains("json") {
            let root: serde_json::Value = serde_json::from_slice(&bytes_vec)?;
            let payload = root
                .get("payload")
                .ok_or_else(|| ApiClientError::Other("missing payload field".to_string()))?
                .clone();

            if payload.is_string() {
                if TypeId::of::<T>() == TypeId::of::<String>() {
                    let s = payload.as_str().unwrap().to_string();
                    let t = serde_json::from_value::<T>(serde_json::Value::String(s))?;
                    return Ok(t);
                } else {
                    return Err(ApiClientError::Other(
                        "payload is a raw string; cannot deserialize into requested type"
                            .to_string(),
                    ));
                }
            }

            let t = serde_json::from_value::<T>(payload)?;
            return Ok(t);
        }

        if content_type.contains("yaml") || content_type.contains("yml") {
            let root: serde_yaml::Value = serde_yaml::from_slice(&bytes_vec)?;
            let payload = root
                .get("payload")
                .ok_or_else(|| ApiClientError::Other("missing payload field".to_string()))?
                .clone();

            let t = serde_yaml::from_value::<T>(payload)?;
            return Ok(t);
        }

        if TypeId::of::<T>() == TypeId::of::<String>() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes_vec);
            let json = serde_json::to_string(&b64)?;
            let t = serde_json::from_str::<T>(&json)?;
            return Ok(t);
        }

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
        let jwt = self.generate_jwt()?;
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
        let jwt = self.generate_jwt()?;
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
        let jwt = self.generate_jwt()?;
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
        let jwt = self.generate_jwt()?;
        let resp = self
            .get_default_request(&rel, Method::GET)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;
        let arr: Vec<crate::types::EventResponse> = resp.json().await?;
        Ok(arr)
    }

    pub async fn get(&self, path: &str) -> Result<reqwest::Response, ApiClientError> {
        let rel = path.trim_start_matches('/');
        let jwt = self.generate_jwt()?;
        let resp = self
            .get_default_request(rel, Method::GET)
            .bearer_auth(jwt)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp)
    }

    pub async fn post_json<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<reqwest::Response, ApiClientError> {
        let rel = path.trim_start_matches('/');
        let jwt = self.generate_jwt()?;
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

#[cfg(test)]
mod tests {
    use super::{ApiClient, ApiClientError};
    use base64::{Engine as _, engine::general_purpose};
    use mockito::Server;
    use openssl::rsa::Rsa;

    #[tokio::test]
    async fn test_generate_and_validate_jwt() {
        let mut server = Server::new_async().await;

        // Generate a keypair for the test
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();
        let _public_key_pem = rsa.public_key_to_pem().unwrap();

        // Create a JWKS response
        let n = general_purpose::URL_SAFE_NO_PAD.encode(rsa.n().to_vec());
        let e = general_purpose::URL_SAFE_NO_PAD.encode(rsa.e().to_vec());
        let jwks_body = format!(
            r#"{{"keys": [{{"kty": "RSA", "kid": "test-key", "n": "{}", "e": "{}"}}]}}"#,
            n, e
        );

        let mock = server
            .mock("GET", "/.well-known/jwks")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&jwks_body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let token = client.generate_jwt().unwrap();

        let result = client
            .validate_event_token(ApiClient::get_jwks, &token)
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_jwks_success() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/.well-known/jwks")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"keys": [{"kty": "RSA", "n": "...", "e": "AQAB"}]}"#)
            .create();

        let result = ApiClient::get_jwks(server.url()).await;
        assert!(result.is_ok());
        let jwks = result.unwrap();
        assert_eq!(jwks.keys.len(), 1);
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_jwks_error() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/.well-known/jwks")
            .with_status(500)
            .create();

        let result = ApiClient::get_jwks(server.url()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ApiClientError::Http(_) => {}
            _ => panic!("Expected Http error"),
        }
        mock.assert();
    }

    #[tokio::test]
    async fn test_put_object_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let mock = server
            .mock("PUT", "/ns1/obj1")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let result = client.put_object("ns1", "obj1", Some("v1"), b"hello").await;
        assert!(result.is_ok());
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_object_json_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let body = r#"{"payload": {"foo": "bar"}}"#;
        let mock = server
            .mock("GET", "/ns1/obj1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct MyObj {
            foo: String,
        }

        let result = client.get_object::<MyObj>("ns1", "obj1", None).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            MyObj {
                foo: "bar".to_string()
            }
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_object_yaml_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let body = "payload:\n  foo: bar";
        let mock = server
            .mock("GET", "/ns1/obj1")
            .with_status(200)
            .with_header("content-type", "application/x-yaml")
            .with_body(body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct MyObj {
            foo: String,
        }

        let result = client.get_object::<MyObj>("ns1", "obj1", None).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            MyObj {
                foo: "bar".to_string()
            }
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_object_raw_string_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let body = "hello world";
        let mock = server
            .mock("GET", "/ns1/obj1")
            .with_status(200)
            .with_body(body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let result = client.get_object::<String>("ns1", "obj1", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "aGVsbG8gd29ybGQ=");
        mock.assert();
    }

    #[tokio::test]
    async fn test_event_crud_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let req = crate::types::EventRequest {
            keys: vec!["k1".to_string()],
            notify: crate::types::NotifyRequest {
                r#type: "webhook".to_string(),
                method: "POST".to_string(),
                urls: vec!["http://example.com".to_string()],
            },
            created_at: None,
        };

        let mock_post = server
            .mock("POST", "/events/ns1")
            .with_status(201)
            .with_body(r#"{"id": "ev1"}"#)
            .create();
        let res = client.post_event("ns1", &req).await.unwrap();
        assert_eq!(res.id, "ev1");
        mock_post.assert();

        let mock_put = server
            .mock("PUT", "/events/ns1/ev1")
            .with_status(200)
            .with_body(r#"{"id": "ev1"}"#)
            .create();
        let res = client.put_event("ns1", "ev1", &req).await.unwrap();
        assert_eq!(res.id, "ev1");
        mock_put.assert();

        let mock_list = server
            .mock("GET", "/events/ns1")
            .with_status(200)
            .with_body(r#"[{"namespace": "ns1", "id": "ev1", "keys": ["k1"], "notify": {"type": "webhook", "method": "POST", "urls": ["http://example.com"]}, "created_at": "2023-01-01T00:00:00Z"}]"#)
            .create();
        let res = client.list_events("ns1").await.unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, "ev1");
        mock_list.assert();

        let mock_delete = server
            .mock("DELETE", "/events/ns1/ev1")
            .with_status(204)
            .create();
        client.delete_event("ns1", "ev1").await.unwrap();
        mock_delete.assert();
    }

    #[tokio::test]
    async fn test_get_object_missing_payload() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let body = r#"{"wrong_field": "bar"}"#;
        let mock = server
            .mock("GET", "/ns1/obj1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let result = client
            .get_object::<serde_json::Value>("ns1", "obj1", None)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ApiClientError::Other(e) => assert!(e.contains("missing payload field")),
            _ => panic!("Expected Other error"),
        }
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_object_string_payload_type_mismatch() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let body = r#"{"payload": "just a string"}"#;
        let mock = server
            .mock("GET", "/ns1/obj1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        #[derive(serde::Deserialize, Debug)]
        struct MyObj {
            #[allow(dead_code)]
            foo: String,
        }

        let result = client.get_object::<MyObj>("ns1", "obj1", None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ApiClientError::Other(e) => assert!(e.contains("payload is a raw string")),
            _ => panic!("Expected Other error"),
        }
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_generic_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let mock = server
            .mock("GET", "/some/path")
            .with_status(200)
            .with_body("ok")
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let resp = client.get("/some/path").await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "ok");
        mock.assert();
    }

    #[tokio::test]
    async fn test_post_json_generic_success() {
        let mut server = Server::new_async().await;
        let rsa = Rsa::generate(2048).unwrap();
        let private_key_pem = rsa.private_key_to_pem().unwrap();

        let mock = server
            .mock("POST", "/some/path")
            .with_status(200)
            .match_body(mockito::Matcher::JsonString(
                r#"{"hello":"world"}"#.to_string(),
            ))
            .create();

        let mut client = ApiClient::new(private_key_pem, "test-key", "object-registry");
        client.base_url = server.url();

        let body = serde_json::json!({"hello": "world"});
        let resp = client.post_json("/some/path", &body).await.unwrap();
        assert_eq!(resp.status(), 200);
        mock.assert();
    }
}
