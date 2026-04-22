use std::sync::{Arc, Mutex};

use runtime::{ApiClient, ApiRequest, AssistantEvent, RuntimeError};

#[derive(Clone)]
pub struct SharedApiClient(pub Arc<Mutex<Box<dyn ApiClient + Send + Sync>>>);

impl SharedApiClient {
    #[must_use]
    pub fn new(api_client: impl ApiClient + Send + Sync + 'static) -> Self {
        Self(Arc::new(Mutex::new(Box::new(api_client))))
    }
}

impl ApiClient for SharedApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stream(request)
    }
}
