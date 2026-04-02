use std::fmt::{Display, Formatter};

use serde_json::Value;

use crate::state::CrawlState;

#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub goal: String,
    pub max_steps: usize,
    pub crawl_state: CrawlState,
}

#[derive(Debug, Clone, Default)]
pub struct CrawlResult {
    pub summary: String,
    pub extracted_data: Vec<Value>,
    pub steps_executed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHandle {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlError {
    message: String,
}

impl CrawlError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for CrawlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CrawlError {}

#[allow(async_fn_in_trait)]
pub trait CrawlAgent {
    async fn run(&mut self, goal: &str) -> Result<CrawlResult, CrawlError>;

    fn fork(&mut self) -> Result<AgentHandle, CrawlError> {
        unimplemented!("forking is not implemented yet; this is a design stub")
    }
}
