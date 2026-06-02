use std::collections::HashMap;

use serde_json::Value;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

pub type ToolHandler = Box<dyn Fn(&Value) -> Result<ToolEffect, ToolExecutionError> + Send + Sync>;

const ASYNC_TOOLS: &[&str] = &[
    "navigate",
    "click",
    "fill_form",
    "page_map",
    "read_content",
    "screenshot",
    "go_back",
    "scroll",
    "wait",
    "select_option",
    "execute_js",
    "hover",
    "press_key",
    "switch_tab",
    "list_resources",
    "save_file",
    "read_pdf",
    "read_document",
    "read_spreadsheet",
    "view_image",
    "transcribe_media",
    "list_archive",
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
        Self::new_with_options(true)
    }

    /// Create a registry with all 27 core tools.
    /// `is_interactive` controls whether `wait_for_human` is allowed to pause.
    #[must_use]
    pub fn new_with_options(is_interactive: bool) -> Self {
        let mut registry = Self::new();
        for &name in ASYNC_TOOLS {
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
        registry.register(
            "wait_for_human",
            Box::new(move |input| crate::tools::wait_for_human::execute(input, is_interactive)),
        );
        registry
    }

    /// Create a registry for child/sub-agents — excludes `wait_for_human`
    /// since children cannot request human intervention.
    #[must_use]
    pub fn new_for_child() -> Self {
        let mut registry = Self::new();
        for &name in ASYNC_TOOLS {
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
        registry
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
    ) -> Result<ToolEffect, ToolExecutionError> {
        match name {
            "navigate" => crate::tools::navigate::execute(input, browser).await,
            "click" => crate::tools::click::execute(input, browser).await,
            "fill_form" => crate::tools::fill_form::execute(input, browser).await,
            "page_map" => crate::tools::page_map::execute(input, browser).await,
            "read_content" => crate::tools::read_content::execute(input, browser).await,
            "screenshot" => crate::tools::screenshot::execute(input, browser).await,
            "go_back" => crate::tools::go_back::execute(input, browser).await,
            "scroll" => crate::tools::scroll::execute(input, browser).await,
            "wait" => crate::tools::wait::execute(input, browser).await,
            "select_option" => crate::tools::select_option::execute(input, browser).await,
            "execute_js" => crate::tools::execute_js::execute(input, browser).await,
            "hover" => crate::tools::hover::execute(input, browser).await,
            "press_key" => crate::tools::press_key::execute(input, browser).await,
            "switch_tab" => crate::tools::switch_tab::execute(input, browser).await,
            "list_resources" => crate::tools::list_resources::execute(input, browser).await,
            "save_file" => crate::tools::save_file::execute(input, browser).await,
            "read_pdf" => crate::tools::read_pdf::execute(input, browser).await,
            "read_document" => crate::tools::read_document::execute(input, browser).await,
            "read_spreadsheet" => crate::tools::read_spreadsheet::execute(input, browser).await,
            "view_image" => crate::tools::view_image::execute(input, browser).await,
            "transcribe_media" => crate::tools::transcribe_media::execute(input, browser).await,
            "list_archive" => crate::tools::list_archive::execute(input, browser).await,
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
    fn new_with_core_tools_registers_all_twenty_seven() {
        let registry = ToolRegistry::new_with_core_tools();
        let effect_tools = [
            "fork",
            "wait_for_subagents",
            "cancel_subagent",
            "subagent_status",
            "wait_for_human",
        ];
        assert_eq!(registry.len(), 27);
        for &name in ASYNC_TOOLS.iter().chain(effect_tools.iter()) {
            assert!(registry.contains(name), "missing core tool: {name}");
        }
    }

    #[test]
    fn new_for_child_excludes_wait_for_human() {
        let registry = ToolRegistry::new_for_child();
        assert_eq!(registry.len(), 20);
        assert!(!registry.contains("wait_for_human"));
        assert!(registry.contains("fork"));
        assert!(registry.contains("navigate"));
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
