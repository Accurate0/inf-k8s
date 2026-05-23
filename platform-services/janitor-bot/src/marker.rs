use std::fmt;

const PREFIX: &str = "janitor-bot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    feature: &'static str,
    key: Option<String>,
}

impl Marker {
    pub fn feature(feature: &'static str) -> Self {
        Self { feature, key: None }
    }

    pub fn keyed(feature: &'static str, key: impl Into<String>) -> Self {
        Self {
            feature,
            key: Some(key.into()),
        }
    }
}

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(key) => write!(f, "<!-- {PREFIX}:{}:{key} -->", self.feature),
            None => write!(f, "<!-- {PREFIX}:{} -->", self.feature),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_feature_marker() {
        assert_eq!(
            Marker::feature("argocd-diff").to_string(),
            "<!-- janitor-bot:argocd-diff -->"
        );
    }

    #[test]
    fn renders_keyed_marker() {
        assert_eq!(
            Marker::keyed("stale", "warned").to_string(),
            "<!-- janitor-bot:stale:warned -->"
        );
    }
}
