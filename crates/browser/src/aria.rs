/// Build a JavaScript string that extracts an ARIA accessibility tree snapshot
/// from the current page. The script walks the DOM, computing roles, accessible
/// names, and states for each element, and returns a JSON tree.
///
/// Parameters:
/// - `scope`: Optional CSS selector to scope the tree to a specific element.
///   When `None`, the entire document body is walked.
/// - `depth`: Maximum nesting depth for the tree (default: 10).
#[must_use]
pub fn build_aria_snapshot_script(scope: Option<&str>, depth: u32) -> String {
    let scope_selector = scope.unwrap_or("body");
    // Escape backticks and backslashes in the scope selector for the JS template literal
    let scope_escaped = scope_selector.replace('\\', "\\\\").replace('`', "\\`");

    format!(
        r#"(function() {{
  let refCounter = 0;

  function getRole(el) {{
    const explicitRole = el.getAttribute('role');
    if (explicitRole) return explicitRole;

    const tag = el.tagName.toLowerCase();
    const type = (el.getAttribute('type') || '').toLowerCase();
    const mapping = {{
      'h1': 'heading', 'h2': 'heading', 'h3': 'heading',
      'h4': 'heading', 'h5': 'heading', 'h6': 'heading',
      'a': 'link', 'button': 'button', 'input': 'textbox',
      'textarea': 'textbox', 'select': 'combobox',
      'ul': 'list', 'ol': 'list', 'li': 'listitem',
      'nav': 'navigation', 'header': 'banner',
      'main': 'main', 'footer': 'contentinfo',
      'img': 'img', 'form': 'form', 'table': 'table',
      'th': 'columnheader', 'td': 'cell', 'tr': 'row',
      'p': 'paragraph', 'span': 'generic', 'div': 'generic',
      'section': 'region', 'article': 'article',
      'aside': 'complementary', 'label': 'label',
      'iframe': 'iframe', 'video': 'video', 'audio': 'audio',
    }};
    if (tag === 'input') {{
      if (type === 'checkbox') return 'checkbox';
      if (type === 'radio') return 'radio';
      if (type === 'submit' || type === 'button' || type === 'reset') return 'button';
      if (type === 'search') return 'searchbox';
      if (type === 'email') return 'textbox';
      if (type === 'number') return 'spinbutton';
      if (type === 'range') return 'slider';
      if (type === 'password') return 'textbox';
    }}
    return mapping[tag] || 'generic';
  }}

  function getName(el) {{
    let name = el.getAttribute('aria-label') ||
      el.getAttribute('title') ||
      el.getAttribute('alt') ||
      el.getAttribute('placeholder') || '';
    if (!name && el.tagName.toLowerCase() === 'a') {{
      name = (el.textContent || '').trim();
    }}
    if (!name && el.tagName.toLowerCase() === 'button') {{
      name = (el.textContent || '').trim();
    }}
    if (!name && (el.tagName.toLowerCase() === 'input' || el.tagName.toLowerCase() === 'textarea')) {{
      name = el.getAttribute('name') || el.getAttribute('id') || '';
    }}
    if (!name) {{
      // For generic elements, use the first 50 chars of trimmed text content
      const text = (el.textContent || '').trim();
      if (text.length <= 50) {{
        name = text;
      }}
    }}
    // Truncate long names
    return name.substring(0, 100);
  }}

  function getStates(el) {{
    const states = [];
    if (el.disabled) states.push('disabled');
    if (el.checked) states.push('checked');
    if (el.getAttribute('aria-checked') === 'true') states.push('checked');
    if (el.getAttribute('aria-expanded') === 'true') states.push('expanded');
    if (el.getAttribute('aria-pressed') === 'true') states.push('pressed=true');
    if (el.getAttribute('aria-invalid') === 'true') states.push('invalid');
    if (el.getAttribute('aria-selected') === 'true') states.push('selected');
    if (el.getAttribute('aria-required') === 'true') states.push('required');
    if (el.tagName.toLowerCase().match(/^h[1-6]$/)) {{
      states.push('level=' + el.tagName[1]);
    }}
    if (el.getAttribute('aria-level')) {{
      states.push('level=' + el.getAttribute('aria-level'));
    }}
    if (document.activeElement === el) states.push('active');
    return states;
  }}

  function shouldInclude(el) {{
    // Skip script, style, noscript, template, and hidden elements
    const tag = el.tagName.toLowerCase();
    if (tag === 'script' || tag === 'style' || tag === 'noscript' ||
        tag === 'template' || tag === 'link' || tag === 'meta' ||
        tag === 'br' || tag === 'hr') {{
      return false;
    }}
    if (el.getAttribute('aria-hidden') === 'true') return false;
    if (el.hidden) return false;
    // Skip empty text nodes
    if (el.nodeType === 3) {{
      const text = (el.textContent || '').trim();
      return text.length > 0;
    }}
    return true;
  }}

  function walk(el, maxDepth) {{
    if (maxDepth < 0) return null;
    if (!shouldInclude(el)) return null;

    const role = getRole(el);
    const name = getName(el);
    const states = getStates(el);
    const ref = refCounter++;

    const children = [];
    if (maxDepth > 0) {{
      for (const child of el.children) {{
        const result = walk(child, maxDepth - 1);
        if (result) children.push(result);
      }}
    }}

    // Include significant text-only nodes as children of generic elements
    if (children.length === 0 && maxDepth > 0 && role === 'generic') {{
      const text = (el.textContent || '').trim();
      // If this element has significant text, don't create extra text children
      // Just return with the name set
    }}

    return {{ role, name, states, ref, children }};
  }}

  let scopeEl;
  try {{
    scopeEl = document.querySelector(`{scope_escaped}`);
  }} catch(e) {{
    return JSON.stringify({{ error: 'scope selector failed: ' + e.message }});
  }}
  if (!scopeEl) {{
    return JSON.stringify({{ error: 'scope element not found: {scope_escaped}' }});
  }}

  const tree = walk(scopeEl, {depth});
  const stats = {{
    total_refs: refCounter,
    max_depth: {depth},
    scope: '{scope_selector}',
    page_url: window.location.href,
    page_title: document.title,
  }};
  return JSON.stringify({{ tree, stats }});
}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_script_with_default_scope() {
        let script = build_aria_snapshot_script(None, 10);
        assert!(script.contains("document.querySelector(`body`)"));
        assert!(script.contains("function walk(el, maxDepth)"));
        assert!(script.contains("refCounter"));
        assert!(script.contains("getRole"));
        assert!(script.contains("getName"));
        assert!(script.contains("getStates"));
        assert!(script.contains("page_url"));
        assert!(script.contains("page_title"));
    }

    #[test]
    fn builds_script_with_custom_scope() {
        let script = build_aria_snapshot_script(Some("#main-content"), 3);
        assert!(script.contains("document.querySelector(`#main-content`)"));
        assert!(script.contains("function walk(el, maxDepth)"));
    }

    #[test]
    fn builds_script_with_zero_depth() {
        let script = build_aria_snapshot_script(None, 0);
        assert!(script.contains("function walk(el, maxDepth)"));
        // Depth 0 is passed as value, verify it compiles
        assert!(script.len() > 100);
    }

    #[test]
    fn escapes_backticks_in_scope() {
        let script = build_aria_snapshot_script(Some("div[data-x='foo`bar']"), 5);
        assert!(script.contains("foo\\`bar"));
    }
}
