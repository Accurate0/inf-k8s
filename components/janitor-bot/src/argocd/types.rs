use serde::Deserialize;

#[derive(Deserialize)]
pub struct Application {
    pub metadata: Metadata,
    pub spec: AppSpec,
}

#[derive(Deserialize)]
pub struct Metadata {
    pub name: String,
}

#[derive(Deserialize)]
pub struct AppSpec {
    #[serde(default)]
    pub sources: Vec<Source>,
}

#[derive(Deserialize)]
pub struct Source {
    #[serde(rename = "targetRevision")]
    pub target_revision: Option<String>,
    pub chart: Option<String>,
}

pub struct SourceDiff {
    pub app_name: String,
    pub chart_name: String,
    pub old_revision: String,
    pub new_revision: String,
    pub source_position: usize,
}
