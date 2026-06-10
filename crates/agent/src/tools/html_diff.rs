use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ContentSection {
    pub heading: String,
    pub hash: u64,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct HtmlDiffTracker {
    /// Maps URL → Vec of previously seen sections
    cached: HashMap<String, Vec<ContentSection>>,
}

impl HtmlDiffTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Split content by heading boundaries and cache it for URL.
    /// Returns the sections for further use.
    pub fn update(&mut self, url: &str, content: &str) -> Vec<ContentSection> {
        let sections = split_into_sections(content);
        self.cached.insert(url.to_string(), sections.clone());
        sections
    }

    /// Returns `Some(diff_output)` if we have a previous version to diff against.
    /// Returns `None` if this is the first visit (caller should use full content).
    pub fn diff(&mut self, url: &str, new_content: &str) -> Option<String> {
        let prev = self.cached.get(url)?.clone();
        let new_sections = split_into_sections(new_content);

        let mut output_parts = Vec::new();
        let mut unchanged_run = 0usize;

        for (i, new_sec) in new_sections.iter().enumerate() {
            let prev_hash = prev.get(i).map(|s| s.hash);
            if prev_hash == Some(new_sec.hash) {
                unchanged_run += 1;
            } else {
                if unchanged_run > 0 {
                    output_parts.push(format!("[unchanged: {unchanged_run} sections]"));
                    unchanged_run = 0;
                }
                output_parts.push(new_sec.content.clone());
            }
        }

        if unchanged_run > 0 {
            output_parts.push(format!("[unchanged: {unchanged_run} sections]"));
        }

        self.cached.insert(url.to_string(), new_sections);

        Some(output_parts.join("\n\n"))
    }
}

fn hash_str(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn split_into_sections(content: &str) -> Vec<ContentSection> {
    let mut sections = Vec::new();
    let mut current_heading = String::new();
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if line.starts_with('#') {
            if !current_lines.is_empty() || !current_heading.is_empty() {
                let content_str = current_lines.join("\n");
                sections.push(ContentSection {
                    heading: current_heading.clone(),
                    hash: hash_str(&content_str),
                    content: if current_heading.is_empty() {
                        content_str
                    } else {
                        format!("{current_heading}\n{content_str}")
                    },
                });
                current_lines.clear();
            }
            current_heading = line.to_string();
        } else {
            current_lines.push(line);
        }
    }

    if !current_lines.is_empty() || !current_heading.is_empty() {
        let content_str = current_lines.join("\n");
        sections.push(ContentSection {
            heading: current_heading.clone(),
            hash: hash_str(&content_str),
            content: if current_heading.is_empty() {
                content_str
            } else {
                format!("{current_heading}\n{content_str}")
            },
        });
    }

    if sections.is_empty() && !content.is_empty() {
        sections.push(ContentSection {
            heading: String::new(),
            hash: hash_str(content),
            content: content.to_string(),
        });
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_visit_returns_none_no_diff() {
        let mut tracker = HtmlDiffTracker::new();
        let result = tracker.diff("https://example.com", "# Hello\nWorld");
        assert!(result.is_none(), "First visit should return None");
    }

    #[test]
    fn second_visit_unchanged_returns_unchanged_marker() {
        let mut tracker = HtmlDiffTracker::new();
        let content = "# Section 1\nContent 1\n# Section 2\nContent 2";
        tracker.update("https://example.com", content);
        let diff = tracker.diff("https://example.com", content).unwrap();
        assert!(
            diff.contains("[unchanged:"),
            "Should have unchanged markers for identical content"
        );
        assert!(
            !diff.contains("Content 1"),
            "Unchanged content should not appear"
        );
    }

    #[test]
    fn second_visit_changed_section_returned() {
        let mut tracker = HtmlDiffTracker::new();
        tracker.update(
            "https://example.com",
            "# Section 1\nOld\n# Section 2\nUnchanged",
        );
        let new_content = "# Section 1\nNew\n# Section 2\nUnchanged";
        let diff = tracker.diff("https://example.com", new_content).unwrap();
        assert!(diff.contains("New"), "Changed section should appear");
        assert!(
            diff.contains("[unchanged:"),
            "Unchanged section should be marker"
        );
    }

    #[test]
    fn diff_output_smaller_than_full_content() {
        let mut tracker = HtmlDiffTracker::new();
        let sections: Vec<String> = (0..10)
            .map(|i| format!("# Section {i}\nContent {i}"))
            .collect();
        let full_content = sections.join("\n");
        tracker.update("https://example.com", &full_content);
        let mut new_sections = sections.clone();
        new_sections[0] = "# Section 0\nNew Content".to_string();
        let new_content = new_sections.join("\n");
        let diff = tracker.diff("https://example.com", &new_content).unwrap();
        assert!(
            diff.len() < new_content.len(),
            "Diff output should be smaller than full content"
        );
    }

    #[test]
    fn content_without_headings_is_single_section() {
        let sections = split_into_sections("plain text only");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "");
        assert_eq!(sections[0].content, "plain text only");
    }
}
