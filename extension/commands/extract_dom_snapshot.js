'use strict';

function extractDomSnapshotScript(payload) {
  const scope = payload?.scope || null;
  let root = document;
  if (scope) {
    const scoped = document.querySelector(scope);
    if (!scoped) {
      return { elements: [] };
    }
    root = scoped;
  }

  function cssPath(el) {
    if (el.id) return '#' + CSS.escape(el.id);
    const parts = [];
    let cur = el;
    while (cur && cur !== document.body && cur !== document.documentElement) {
      let seg = cur.tagName.toLowerCase();
      const parent = cur.parentElement;
      if (parent) {
        const sibs = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
        if (sibs.length > 1) seg += ':nth-of-type(' + (sibs.indexOf(cur) + 1) + ')';
      }
      parts.unshift(seg);
      cur = cur.parentElement;
    }
    return parts.join(' > ');
  }

  function getLabelledByText(el) {
    const labelledBy = el.getAttribute('aria-labelledby');
    if (!labelledBy) return null;
    const text = labelledBy
      .split(/\s+/)
      .filter(Boolean)
      .map((id) => document.getElementById(id)?.innerText?.trim() || '')
      .filter(Boolean)
      .join(' ')
      .trim();
    return text || null;
  }

  function isVisible(el) {
    const style = getComputedStyle(el);
    return el.offsetParent !== null && !el.hidden && style.visibility !== 'hidden';
  }

  function isFloating(el) {
    const style = getComputedStyle(el);
    return ['fixed', 'absolute'].includes(style.position) && Number.parseInt(style.zIndex || '0', 10) > 0;
  }

  const selectors = [
    'button', 'input', 'select', 'textarea',
    '[role="combobox"]', '[role="listbox"]', '[role="option"]',
    '[role="menuitem"]', '[role="treeitem"]', '[role="tab"]',
    '[role="menu"]', '[role="menubar"]', 'li[role]',
    '[aria-expanded]', '[aria-controls]', '[aria-owns]',
    '[popover]', '[aria-haspopup]'
  ];

  const seen = new Set();
  const elements = [];
  for (const el of root.querySelectorAll(selectors.join(','))) {
    const selector = cssPath(el);
    if (seen.has(selector)) continue;
    seen.add(selector);
    elements.push({
      tag: el.tagName.toLowerCase(),
      role: el.getAttribute('role'),
      aria_expanded: el.getAttribute('aria-expanded'),
      aria_selected: el.getAttribute('aria-selected'),
      aria_pressed: el.getAttribute('aria-pressed'),
      aria_controls: el.getAttribute('aria-controls'),
      aria_owns: el.getAttribute('aria-owns'),
      text: el.innerText?.trim()?.slice(0, 120) || null,
      aria_label: el.getAttribute('aria-label'),
      aria_labelledby_text: getLabelledByText(el),
      title: el.getAttribute('title'),
      placeholder: el.getAttribute('placeholder'),
      name: el.getAttribute('name'),
      visible: isVisible(el),
      floating: isFloating(el),
      selector,
    });
  }

  return { elements };
}

async function handleExtractDomSnapshot(tabId, payload) {
  await ensureAttached(tabId);

  const evalPayload = {
    scope: payload?.scope || null,
  };

  const expression = `(${extractDomSnapshotScript.toString()})(${JSON.stringify(evalPayload)})`;
  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'extract_dom_snapshot script threw exception');
  }

  return res.result?.value || { elements: [] };
}
