use crate::event::WorkflowEvent;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

pub fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let Some(hex_sig) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(decoded) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&decoded).is_ok()
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    name: Option<String>,
    conclusion: Option<String>,
    html_url: Option<String>,
    head_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRepository {
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunPayload {
    workflow_run: Option<WorkflowRun>,
    repository: Option<WorkflowRepository>,
}

pub fn parse_workflow_event(body: &[u8]) -> Option<WorkflowEvent> {
    let payload: WorkflowRunPayload = serde_json::from_slice(body).ok()?;
    let run = payload.workflow_run?;
    Some(WorkflowEvent {
        workflow_name: run.name?,
        conclusion: run.conclusion.unwrap_or_default(),
        run_url: run.html_url?,
        repository: payload.repository?.full_name?,
        branch: run.head_branch.unwrap_or_default(),
    })
}
