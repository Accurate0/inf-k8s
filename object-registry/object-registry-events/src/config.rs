use config::{Config, ConfigError, File, FileFormat};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Notify {
    #[serde(rename = "HTTP")]
    HTTP {
        method: String,
        urls: Vec<String>,
        audience: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
pub struct Event {
    pub namespace: String,
    pub keys: Vec<String>,
    pub notify: Notify,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EventsConfig {
    pub events: Vec<Event>,
}

impl EventsConfig {
    pub fn new() -> Result<Self, ConfigError> {
        let file_str = include_str!("../config.yaml");
        let file = File::from_str(file_str, FileFormat::Yaml).required(true);

        let s = Config::builder().add_source(file).build()?;

        s.try_deserialize()
    }
}
