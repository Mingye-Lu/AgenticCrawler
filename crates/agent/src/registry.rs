use std::collections::HashMap;

use serde_json::Value;

use crate::{BrowserContext, CrawlState};
use crate::{ToolEffect, ToolExecutionError};

pub type ToolHandler = Box<dyn Fn(&Value) -> Result<ToolEffect, ToolExecutionError> + Send + Sync>;

const ASYNC_TOOLS: [&str; 31] = [
    "navigate",
    "click",
    "click_at",
    "fill_form",
    "page_map",
    "read_content",
    "list_network_activity",
    "inspect_request",
    "list_websocket_activity",
    "inspect_websocket",
    "screenshot",
    "go_back",
    "refresh",
    "scroll",
    "wait",
    "select_option",
    "execute_js",
    "hover",
    "press_key",
    "switch_tab",
    "list_resources",
    "list_page_logs",
    "inspect_log",
    "save_file",
    "set_device",
    "get_page_performance",
    "inspect_cookies",
    "inspect_storage",
    "measure_coverage",
    "audit_accessibility",
    "intercept_network",
];

#[derive(Default)]
pub struct ToolRegistry {
    handlers: HashMap<String, ToolHandler>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn new_with_core_tools() -> Self {
        let mut registry = Self::new();
        for name in ASYNC_TOOLS {
            let tool_name = name.to_string();
            registry.register(
                name,
                Box::new(move |_| Err(ToolExecutionError::requires_async(tool_name.clone()))),
            );
        }
        registry.register("fork", Box::new(crate::tools::fork::execute));
        registry.register(
            "wait_for_subagents",
            Box::new(crate::tools::wait_for_subagents::execute),
        );
        registry.register(
            "cancel_subagent",
            Box::new(crate::tools::cancel_subagent::execute),
        );
        registry.register(
            "subagent_status",
            Box::new(crate::tools::subagent_status::execute),
        );
        // Script management tools (sync, no browser needed)
        registry.register("run_script", Box::new(crate::tools::run_script::execute));
        registry.register("save_script", Box::new(crate::tools::save_script::execute));
        registry.register(
            "list_scripts",
            Box::new(crate::tools::list_scripts::execute),
        );
        registry.register("read_script", Box::new(crate::tools::read_script::execute));
        registry.register(
            "script_status",
            Box::new(crate::tools::script_status::execute),
        );
        registry.register(
            "wait_for_scripts",
            Box::new(crate::tools::wait_for_scripts::execute),
        );
        registry.register(
            "cancel_script",
            Box::new(crate::tools::cancel_script::execute),
        );
        registry
    }

    /// Create a registry for child/sub-agents (same tool set as parent).
    #[must_use]
    pub fn new_for_child() -> Self {
        Self::new_with_core_tools()
    }

    pub fn register(&mut self, name: impl Into<String>, handler: ToolHandler) {
        self.handlers.insert(name.into(), handler);
    }

    #[must_use]
    pub fn is_async_tool(name: &str) -> bool {
        ASYNC_TOOLS.contains(&name)
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

    pub async fn execute_async(
        &self,
        name: &str,
        input: &Value,
        browser: &mut BrowserContext,
        crawl_state: &mut CrawlState,
    ) -> Result<ToolEffect, ToolExecutionError> {
        match name {
            "navigate" => crate::tools::navigate::execute(input, browser, crawl_state).await,
            "click" => crate::tools::click::execute(input, browser, crawl_state).await,
            "click_at" => crate::tools::click_at::execute(input, browser, crawl_state).await,
            "fill_form" => crate::tools::fill_form::execute(input, browser, crawl_state).await,
            "page_map" => crate::tools::page_map::execute(input, browser, crawl_state).await,
            "read_content" => {
                crate::tools::read_content::execute(input, browser, crawl_state).await
            }
            "list_network_activity" => {
                crate::tools::network_activity::list_network_activity(input, browser, crawl_state)
                    .await
            }
            "inspect_request" => {
                crate::tools::network_activity::inspect_request(input, browser, crawl_state)
            }
            "list_websocket_activity" => {
                crate::tools::websocket_activity::list_websocket_activity(
                    input,
                    browser,
                    crawl_state,
                )
                .await
            }
            "inspect_websocket" => {
                crate::tools::websocket_activity::inspect_websocket(input, browser, crawl_state)
            }
            "screenshot" => crate::tools::screenshot::execute(input, browser).await,
            "go_back" => crate::tools::go_back::execute(input, browser, crawl_state).await,
            "refresh" => crate::tools::refresh::execute(input, browser, crawl_state).await,
            "scroll" => crate::tools::scroll::execute(input, browser, crawl_state).await,
            "wait" => crate::tools::wait::execute(input, browser).await,
            "select_option" => {
                crate::tools::select_option::execute(input, browser, crawl_state).await
            }
            "execute_js" => crate::tools::execute_js::execute(input, browser, crawl_state).await,
            "hover" => crate::tools::hover::execute(input, browser, crawl_state).await,
            "press_key" => crate::tools::press_key::execute(input, browser, crawl_state).await,
            "switch_tab" => crate::tools::switch_tab::execute(input, browser).await,
            "list_resources" => crate::tools::list_resources::execute(input, browser).await,
            "list_page_logs" => {
                crate::tools::page_logs::execute_list_page_logs(input, browser, crawl_state).await
            }
            "inspect_log" => crate::tools::page_logs::execute_inspect_log(input, crawl_state),
            "save_file" => crate::tools::save_file::execute(input, browser).await,
            "set_device" => crate::tools::set_device::execute(input, browser, crawl_state).await,
            "get_page_performance" => crate::tools::page_performance::execute(input, browser).await,
            "inspect_cookies" => {
                crate::tools::storage_inspect::inspect_cookies(input, browser).await
            }
            "inspect_storage" => {
                crate::tools::storage_inspect::inspect_storage(input, browser).await
            }
            "measure_coverage" => crate::tools::coverage::execute(input, browser).await,
            "audit_accessibility" => crate::tools::accessibility::execute(input, browser).await,
            "intercept_network" => {
                crate::tools::intercept_network::execute(input, browser, crawl_state).await
            }
            _ => {
                if let Some(handler) = self.handlers.get(name) {
                    handler(input)
                } else {
                    Err(ToolExecutionError::new(format!("unknown tool: `{name}`")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_core_tools_registers_all_forty_two() {
        let registry = ToolRegistry::new_with_core_tools();
        let effect_tools = [
            "fork",
            "wait_for_subagents",
            "cancel_subagent",
            "subagent_status",
        ];
        let script_tools = [
            "run_script",
            "save_script",
            "list_scripts",
            "read_script",
            "script_status",
            "wait_for_scripts",
            "cancel_script",
        ];
        assert_eq!(registry.len(), 42);
        for &name in ASYNC_TOOLS
            .iter()
            .chain(effect_tools.iter())
            .chain(script_tools.iter())
        {
            assert!(registry.contains(name), "missing core tool: {name}");
        }
    }

    #[test]
    fn new_for_child_same_as_parent() {
        let registry = ToolRegistry::new_for_child();
        assert_eq!(registry.len(), 42);
        assert!(registry.contains("fork"));
        assert!(registry.contains("navigate"));
        assert!(registry.contains("list_scripts"));
    }

    #[test]
    fn sync_handler_for_core_tool_returns_error_hint() {
        let registry = ToolRegistry::new_with_core_tools();
        let handler = registry.get("navigate").unwrap();
        let err = handler(&serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("async"));
    }

    #[test]
    fn contains_returns_false_for_unknown_tool() {
        let registry = ToolRegistry::new_with_core_tools();
        assert!(!registry.contains("nonexistent"));
    }
}
