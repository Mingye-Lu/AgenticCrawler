use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{future::Future, pin::Pin};

use script::grammar::ScriptNode;
use serde_json::Value;
use tokio::task::JoinSet;

use super::{ScriptExecutionError, ScriptExecutor};
use crate::BrowserContext;

#[derive(Clone)]
struct ParallelBranchResult {
    index: usize,
    extracted_data: Vec<Value>,
    errors_caught: usize,
}

impl ScriptExecutor {
    pub(super) fn execute_parallel<'a>(
        &'a mut self,
        branches: &'a [Vec<ScriptNode>],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            if branches.len() > self.limits.max_parallel_branches {
                return Err(ScriptExecutionError::ToolError(format!(
                    "parallel branch count {} exceeds limit {}",
                    branches.len(),
                    self.limits.max_parallel_branches
                )));
            }

            if branches.is_empty() {
                return Ok(());
            }

            let pre_parallel_bytes = self.output_bytes.load(Ordering::Relaxed);

            let shared_bridge = self.browser.bridge().clone();
            let current_url = self.state.current_url.clone();
            let mut page_indices = Vec::with_capacity(branches.len());
            let mut join_set = JoinSet::new();

            for (index, branch) in branches.iter().enumerate() {
                let page_index = {
                    let mut bridge = shared_bridge.lock().await;
                    bridge
                        .new_page(current_url.as_deref())
                        .await
                        .map_err(|error| ScriptExecutionError::ToolError(error.to_string()))?
                };
                page_indices.push(page_index);

                let mut browser = BrowserContext::new_shared(shared_bridge.clone(), page_index);
                if let Some(url) = current_url.as_deref() {
                    browser.set_navigated_url(url, true);
                }

                let branch_executor = Self {
                    browser,
                    crawl_state: self.crawl_state.clone(),
                    state: self.state.clone(),
                    shared_state: self.shared_state.clone(),
                    limits: self.limits.clone(),
                    output_bytes: self.output_bytes.clone(),
                    variables: self.variables.clone(),
                    extracted_data: Vec::new(),
                    yielded_data: self.yielded_data.clone(),
                    start_time: self.start_time,
                    step_counter: self.step_counter.clone(),
                    cancel_token: self.cancel_token.clone(),
                    registry: Arc::clone(&self.registry),
                };
                let branch_steps = branch.clone();

                join_set.spawn(async move {
                    Self::run_parallel_branch(branch_executor, index, branch_steps).await
                });
            }

            let mut branch_results: Vec<Option<ParallelBranchResult>> = vec![None; branches.len()];
            let mut first_error = None;

            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(Ok(branch_result)) => {
                        let index = branch_result.index;
                        branch_results[index] = Some(branch_result);
                    }
                    Ok(Err(error)) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                            join_set.abort_all();
                        }
                    }
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(ScriptExecutionError::ToolError(format!(
                                "parallel branch task failed: {error}"
                            )));
                            join_set.abort_all();
                        }
                    }
                }
            }

            let cleanup_error = Self::close_parallel_pages(shared_bridge, &page_indices)
                .await
                .err();
            self.state.step = self.step_counter.load(Ordering::Relaxed);

            self.finish_parallel(
                branches.len(),
                branch_results,
                pre_parallel_bytes,
                first_error.or(cleanup_error),
            )
        })
    }

    /// Merges every branch that completed successfully into `self` before deciding
    /// whether to return an error. Branches that finished before a sibling errored (or
    /// before cleanup failed) already did real work — extracted data, caught errors,
    /// output-byte accounting — and must not be silently discarded just because the
    /// overall parallel block is reported as failed. Merging first (rather than bailing
    /// out early) means that data still reaches the caller via `ScriptResult`, which is
    /// built from `self.extracted_data`/`self.state` regardless of success or failure.
    ///
    /// Branches that never finished (aborted after a sibling error) contributed bytes to
    /// the shared budget that are never merged in here, so on failure the budget is reset
    /// to the pre-parallel snapshot plus only the bytes actually kept from completed
    /// branches — not the raw pre-parallel snapshot, which would under-count the data
    /// merged in from siblings that did finish.
    fn finish_parallel(
        &mut self,
        branch_count: usize,
        branch_results: Vec<Option<ParallelBranchResult>>,
        pre_parallel_bytes: usize,
        error: Option<ScriptExecutionError>,
    ) -> Result<(), ScriptExecutionError> {
        if error.is_some() {
            let completed_count = branch_results
                .iter()
                .filter(|result| result.is_some())
                .count();
            if completed_count > 0 {
                eprintln!(
                    "Warning: parallel script block failed after {completed_count}/{branch_count} branch(es) completed successfully; merging their partial results (extracted data, caught errors) instead of discarding them"
                );
            }

            let merged_bytes: usize = branch_results
                .iter()
                .flatten()
                .flat_map(|result| result.extracted_data.iter())
                .map(|value| value.to_string().len())
                .sum();
            self.output_bytes
                .store(pre_parallel_bytes + merged_bytes, Ordering::Relaxed);
        }

        for result in branch_results.into_iter().flatten() {
            self.extracted_data.extend(result.extracted_data);
            self.state.errors_caught += result.errors_caught;
        }
        self.state.items_collected = self.extracted_data.len();

        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn run_parallel_branch(
        mut executor: ScriptExecutor,
        index: usize,
        branch_steps: Vec<ScriptNode>,
    ) -> Result<ParallelBranchResult, ScriptExecutionError> {
        for step in &branch_steps {
            executor.check_limits()?;
            executor.execute_node(step).await?;
            executor.state.step = executor.step_counter.load(Ordering::Relaxed);
            executor.state.elapsed_secs = executor.start_time.elapsed().as_secs_f64();
            executor.sync_shared_state();
        }

        Ok(ParallelBranchResult {
            index,
            extracted_data: executor.extracted_data,
            errors_caught: executor.state.errors_caught,
        })
    }

    async fn close_parallel_pages(
        shared_bridge: crate::SharedBridge,
        page_indices: &[usize],
    ) -> Result<(), ScriptExecutionError> {
        let mut cleanup_errors = Vec::new();

        for &page_index in page_indices {
            if let Err(error) = shared_bridge.lock().await.close_page(page_index).await {
                cleanup_errors.push(format!("page {page_index}: {error}"));
            }
        }

        if cleanup_errors.is_empty() {
            Ok(())
        } else {
            Err(ScriptExecutionError::ToolError(format!(
                "failed to close parallel pages: {}",
                cleanup_errors.join(", ")
            )))
        }
    }
}
