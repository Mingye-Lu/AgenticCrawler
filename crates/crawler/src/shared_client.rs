use std::sync::{Arc, Mutex};

use runtime::{ApiClient, ApiRequest, AssistantEvent, RuntimeError};

/// Thread-safe wrapper that lets parent and child agents share a single
/// `ApiClient` behind an `Arc<Mutex<…>>`.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because
/// `ApiClient::stream()` is a **synchronous** trait method required to be
/// object-safe (`dyn ApiClient`).  The lock is held only for the duration of
/// a single `stream()` call and is never held across `.await` points.
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
            .map_err(|e| RuntimeError::new(format!("SharedApiClient mutex poisoned: {e}")))?
            .stream(request)
    }
}
