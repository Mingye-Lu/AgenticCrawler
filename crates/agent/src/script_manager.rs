use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use acrawl_core::{ScriptLimits, ScriptResult, ScriptState, ScriptStatus, ScriptTask};
use runtime::settings::ScriptSettings;
use script::parser::{parse_script, validate_script};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::script_executor::ScriptExecutor;
use crate::BrowserContext;

#[derive(Debug)]
pub struct RunningScript {
    pub script_id: String,
    pub state: Arc<RwLock<ScriptState>>,
    pub handle: JoinHandle<ScriptResult>,
    pub cancel_token: CancellationToken,
}

#[derive(Debug)]
pub struct ScriptManager {
    pub scripts: HashMap<String, RunningScript>,
    pub settings: ScriptSettings,
    pub max_concurrent: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    NotFound(String),
    ConcurrentLimitExceeded,
    ParseError(String),
    Other(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(script_id) => write!(f, "script `{script_id}` not found"),
            Self::ConcurrentLimitExceeded => write!(f, "script concurrent limit exceeded"),
            Self::ParseError(message) => write!(f, "script parse/validation failed: {message}"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ScriptError {}

impl ScriptManager {
    #[must_use]
    pub fn new(settings: ScriptSettings) -> Self {
        let max_concurrent = settings
            .max_concurrent_scripts
            .or_else(|| ScriptSettings::default().max_concurrent_scripts)
            .unwrap_or(5);

        Self {
            scripts: HashMap::new(),
            settings,
            max_concurrent,
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn spawn_script(
        &mut self,
        task: ScriptTask,
        browser: BrowserContext,
    ) -> Result<String, ScriptError> {
        self.check_can_spawn()?;

        let script_id = self.generate_script_id();
        let limits = self.effective_limits(&task.limits);
        let script_definition = parse_script(&task.script)
            .map_err(|error| ScriptError::ParseError(error.to_string()))?;
        validate_script(&script_definition, &limits).map_err(|errors| {
            let message = errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            ScriptError::ParseError(message)
        })?;

        let initial_state = ScriptState {
            script_id: script_id.clone(),
            status: ScriptStatus::Pending,
            step: 0,
            total_steps: None,
            current_url: None,
            items_collected: 0,
            elapsed_secs: 0.0,
            errors_caught: 0,
            yielded_data: Vec::new(),
        };
        let state = Arc::new(RwLock::new(initial_state));
        let cancel_token = CancellationToken::new();
        let executor = ScriptExecutor::new(
            script_id.clone(),
            browser,
            limits,
            state.clone(),
            cancel_token.clone(),
        );
        let handle = tokio::task::spawn(executor.execute(script_definition));

        self.scripts.insert(
            script_id.clone(),
            RunningScript {
                script_id: script_id.clone(),
                state,
                handle,
                cancel_token,
            },
        );

        Ok(script_id)
    }

    pub fn get_status(&self, script_id: &str) -> Result<ScriptState, ScriptError> {
        let running_script = self
            .scripts
            .get(script_id)
            .ok_or_else(|| ScriptError::NotFound(script_id.to_string()))?;

        running_script
            .state
            .read()
            .map(|state| state.clone())
            .map_err(|error| ScriptError::Other(format!("script state lock poisoned: {error}")))
    }

    pub async fn wait_for_scripts(
        &mut self,
        script_ids: Option<Vec<String>>,
    ) -> Result<Vec<ScriptResult>, ScriptError> {
        let target_ids = script_ids.unwrap_or_else(|| self.scripts.keys().cloned().collect());
        let mut running = Vec::with_capacity(target_ids.len());

        for script_id in target_ids {
            let running_script = self
                .scripts
                .remove(&script_id)
                .ok_or_else(|| ScriptError::NotFound(script_id.clone()))?;
            running.push(running_script);
        }

        let mut results = Vec::with_capacity(running.len());
        for running_script in running {
            let result = running_script.handle.await.map_err(|error| {
                ScriptError::Other(format!(
                    "script `{}` task join failed: {error}",
                    running_script.script_id
                ))
            })?;
            results.push(result);
        }

        self.cleanup_completed();
        Ok(results)
    }

    pub fn cancel_script(&self, script_id: &str) -> Result<(), ScriptError> {
        let running_script = self
            .scripts
            .get(script_id)
            .ok_or_else(|| ScriptError::NotFound(script_id.to_string()))?;

        running_script.cancel_token.cancel();

        running_script
            .state
            .write()
            .map(|mut state| {
                state.status = ScriptStatus::Cancelled;
            })
            .map_err(|error| ScriptError::Other(format!("script state lock poisoned: {error}")))
    }

    pub fn check_can_spawn(&self) -> Result<(), ScriptError> {
        let active_scripts = self
            .scripts
            .values()
            .filter(|running_script| !running_script.handle.is_finished())
            .count();

        if active_scripts >= self.max_concurrent {
            return Err(ScriptError::ConcurrentLimitExceeded);
        }

        Ok(())
    }

    pub fn cleanup_completed(&mut self) {
        self.scripts
            .retain(|_, running_script| !running_script.handle.is_finished());
    }

    fn generate_script_id(&self) -> String {
        for attempt in 0_u32..1024 {
            let now_nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            now_nanos.hash(&mut hasher);
            self.scripts.len().hash(&mut hasher);
            attempt.hash(&mut hasher);
            #[allow(clippy::cast_possible_truncation)]
            let candidate = format!("scr_{:08x}", (hasher.finish() as u32));

            if !self.scripts.contains_key(&candidate) {
                return candidate;
            }
        }

        format!(
            "scr_{:08x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        )
    }

    fn effective_limits(&self, requested: &ScriptLimits) -> ScriptLimits {
        let defaults = ScriptSettings::default();

        ScriptLimits {
            max_steps: requested.max_steps.min(
                self.settings
                    .max_steps
                    .or(defaults.max_steps)
                    .unwrap_or(requested.max_steps),
            ),
            max_timeout_secs: requested.max_timeout_secs.min(
                self.settings
                    .max_timeout_secs
                    .or(defaults.max_timeout_secs)
                    .unwrap_or(requested.max_timeout_secs),
            ),
            max_output_bytes: requested.max_output_bytes.min(
                self.settings
                    .max_output_bytes
                    .or(defaults.max_output_bytes)
                    .unwrap_or(requested.max_output_bytes),
            ),
            max_script_size_bytes: requested.max_script_size_bytes.min(
                self.settings
                    .max_script_size_bytes
                    .or(defaults.max_script_size_bytes)
                    .unwrap_or(requested.max_script_size_bytes),
            ),
            max_parallel_branches: requested.max_parallel_branches.min(
                self.settings
                    .max_parallel_branches
                    .or(defaults.max_parallel_branches)
                    .unwrap_or(requested.max_parallel_branches),
            ),
            max_nesting_depth: requested.max_nesting_depth.min(
                self.settings
                    .max_nesting_depth
                    .or(defaults.max_nesting_depth)
                    .unwrap_or(requested.max_nesting_depth),
            ),
            per_step_timeout_secs: requested.per_step_timeout_secs.min(
                self.settings
                    .per_step_timeout_secs
                    .or(defaults.per_step_timeout_secs)
                    .unwrap_or(requested.per_step_timeout_secs),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use acrawl_core::{ScriptResult, ScriptState, ScriptStatus};
    use runtime::settings::ScriptSettings;
    use tokio_util::sync::CancellationToken;

    use super::{RunningScript, ScriptManager};

    fn completed_script_entry(id: &str) -> RunningScript {
        let state = Arc::new(RwLock::new(ScriptState {
            script_id: id.to_string(),
            status: ScriptStatus::Completed,
            step: 1,
            total_steps: Some(1),
            current_url: None,
            items_collected: 0,
            elapsed_secs: 0.0,
            errors_caught: 0,
            yielded_data: Vec::new(),
        }));
        let handle = tokio::task::spawn(async {
            ScriptResult {
                script_id: "first".to_string(),
                status: ScriptStatus::Completed,
                extracted_data: vec![],
                yielded_data: vec![],
                steps_executed: 1,
                elapsed_secs: 0.0,
                error: None,
            }
        });
        RunningScript {
            script_id: id.to_string(),
            state,
            handle,
            cancel_token: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn completed_script_survives_subsequent_spawn_check() {
        let mut manager = ScriptManager::new(ScriptSettings::default());
        manager
            .scripts
            .insert("first".to_string(), completed_script_entry("first"));

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(manager.scripts["first"].handle.is_finished());
        assert_eq!(manager.scripts.len(), 1, "completed entry must stay in map");

        manager.check_can_spawn().expect("should be able to spawn");
        assert!(
            manager.scripts.contains_key("first"),
            "check_can_spawn must not evict completed entry"
        );

        let status = manager
            .get_status("first")
            .expect("first must still be retrievable after spawn check");
        assert_eq!(status.status, ScriptStatus::Completed);
    }
}
