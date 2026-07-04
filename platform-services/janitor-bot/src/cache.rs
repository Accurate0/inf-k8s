use std::sync::Arc;

pub struct CacheAccessor {
    pub dashboard: moka::sync::Cache<(), Arc<str>>,
}

impl CacheAccessor {
    pub fn new() -> Self {
        Self {
            dashboard: moka::sync::Cache::builder().build(),
        }
    }
}

impl Default for CacheAccessor {
    fn default() -> Self {
        Self::new()
    }
}
