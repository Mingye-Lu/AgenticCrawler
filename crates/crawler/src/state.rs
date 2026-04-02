use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct CrawlState {
    pub current_url: Option<String>,
    pub action_history: Vec<String>,
    pub extracted_data: Vec<Value>,
    pub step_count: usize,
}
