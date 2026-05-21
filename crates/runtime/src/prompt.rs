#[derive(Debug)]
pub enum PromptBuildError {
    Io(std::io::Error),
}

impl std::fmt::Display for PromptBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PromptBuildError {}

impl From<std::io::Error> for PromptBuildError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemPromptBuilder {
    append_sections: Vec<String>,
}

impl SystemPromptBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn append_section(mut self, section: impl Into<String>) -> Self {
        self.append_sections.push(section.into());
        self
    }

    #[must_use]
    pub fn build(&self) -> Vec<String> {
        self.append_sections.clone()
    }

    #[must_use]
    pub fn render(&self) -> String {
        self.build().join("\n\n")
    }
}

#[must_use]
pub fn prepend_bullets(items: Vec<String>) -> Vec<String> {
    items.into_iter().map(|item| format!(" - {item}")).collect()
}

#[cfg(test)]
mod tests {
    use super::SystemPromptBuilder;

    #[test]
    fn build_returns_appended_sections_only() {
        let sections = SystemPromptBuilder::new()
            .append_section("# First")
            .append_section("# Second")
            .build();

        assert_eq!(sections, vec!["# First", "# Second"]);
    }

    #[test]
    fn render_joins_sections_with_blank_lines() {
        let prompt = SystemPromptBuilder::new()
            .append_section("# First")
            .append_section("# Second")
            .render();

        assert_eq!(prompt, "# First\n\n# Second");
    }
}
