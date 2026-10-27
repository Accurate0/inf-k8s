//! Webhook helpers shared by janitor-bot and `k8sctl janitor`, kept free of server
//! dependencies so the CLI doesn't link git2 or the forge clients.

pub mod github {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    /// Computes the `X-Hub-Signature-256` header value for `body`, as GitHub sends
    /// it: `sha256=<hex HMAC-SHA256(secret, body)>`. Inverse of janitor-bot's
    /// `github::verify_signature`; used by `k8sctl janitor replay` to sign saved
    /// payloads.
    pub fn sign_payload(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts keys of any size");
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    /// Infers the `X-GitHub-Event` header from a raw webhook body by detecting
    /// marker keys, mirroring the dispatch in `handle_github_webhook`. Returns
    /// `None` if the shape matches none of the handled events.
    pub fn infer_event(body: &[u8]) -> Option<&'static str> {
        let v: serde_json::Value = serde_json::from_slice(body).ok()?;
        let obj = v.as_object()?;

        if obj.get("workflow_run").is_some_and(|w| w.is_object()) {
            Some("workflow_run")
        } else if obj.get("check_run").is_some_and(|c| c.is_object()) {
            Some("check_run")
        } else if obj.contains_key("ref")
            && (obj.contains_key("pusher") || obj.contains_key("commits"))
        {
            Some("push")
        } else if obj.contains_key("context")
            && obj.contains_key("state")
            && obj.contains_key("sha")
        {
            Some("status")
        } else {
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn infer_event_detects_each_type() {
            assert_eq!(
                infer_event(br#"{"workflow_run":{"id":1},"action":"completed"}"#),
                Some("workflow_run")
            );
            assert_eq!(
                infer_event(br#"{"check_run":{"id":1},"action":"completed"}"#),
                Some("check_run")
            );
            assert_eq!(
                infer_event(br#"{"ref":"refs/heads/main","pusher":{"name":"x"}}"#),
                Some("push")
            );
            assert_eq!(
                infer_event(br#"{"sha":"abc","state":"failure","context":"ci/test"}"#),
                Some("status")
            );
            assert_eq!(infer_event(br#"{"unrelated":true}"#), None);
            assert_eq!(infer_event(b"not json"), None);
        }
    }
}

pub mod forgejo {
    /// Infers the `X-Forgejo-Event` header (and optional `X-Forgejo-Event-Type`)
    /// from a raw Forgejo webhook body, mirroring the dispatch in
    /// `handle_forgejo_webhook`. Forgejo reports a comment on a PR as event
    /// `issue_comment` with event-type `pull_request_comment`; a comment on a plain
    /// issue has `issue.pull_request` null. Returns `None` if the shape matches none
    /// of the handled events.
    pub fn infer_event(body: &[u8]) -> Option<(&'static str, Option<&'static str>)> {
        let v: serde_json::Value = serde_json::from_slice(body).ok()?;
        let obj = v.as_object()?;

        if obj.contains_key("comment") {
            let on_pr = obj
                .get("issue")
                .and_then(|i| i.get("pull_request"))
                .is_some_and(|pr| !pr.is_null());
            Some(("issue_comment", on_pr.then_some("pull_request_comment")))
        } else if obj.get("pull_request").is_some_and(|p| p.is_object()) {
            Some(("pull_request", None))
        } else if obj.contains_key("ref")
            && (obj.contains_key("pusher") || obj.contains_key("commits"))
        {
            Some(("push", None))
        } else {
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn infer_event_distinguishes_comment_targets() {
            assert_eq!(
                infer_event(br#"{"action":"created","issue":{"pull_request":{}},"comment":{"body":"@janitor merge"}}"#),
                Some(("issue_comment", Some("pull_request_comment")))
            );
            assert_eq!(
                infer_event(br#"{"action":"created","issue":{"pull_request":null},"comment":{"body":"@janitor ack"}}"#),
                Some(("issue_comment", None))
            );
            assert_eq!(
                infer_event(br#"{"action":"opened","pull_request":{"number":5}}"#),
                Some(("pull_request", None))
            );
            assert_eq!(infer_event(br#"{"action":"deleted"}"#), None);
            assert_eq!(infer_event(b"not json"), None);
        }
    }
}
