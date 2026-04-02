use std::collections::HashMap;

use serde_json::Value;

use crate::CrawlError;

pub type ToolHandler = Box<dyn Fn(&Value) -> Result<Value, CrawlError> + Send + Sync>;

#[derive(Default)]
pub struct ToolRegistry {
    handlers: HashMap<String, ToolHandler>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: impl Into<String>, handler: ToolHandler) {
        self.handlers.insert(name.into(), handler);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolHandler> {
        self.handlers.get(name)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}
