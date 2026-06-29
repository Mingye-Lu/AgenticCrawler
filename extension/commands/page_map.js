'use strict';

function pageMapScript(scope) {
  let root = document;
  if (scope) {
    const scoped = document.querySelector(scope);
    if (!scoped) {
      return { scope_not_found: true, scope, headings: [], landmarks: [], forms: [], links: [], interactive: { counts: { buttons: 0, inputs: 0, selects: 0, textareas: 0, total: 0 }, elements: [] }, meta: { title: document.title, description: '', url: window.location.href }, total_landmarks: 0, total_forms: 0, total_links: 0 };
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

  const headings = Array.from(root.querySelectorAll('h1, h2, h3, h4, h5, h6')).map((el) => {
    const level = parseInt(el.tagName[1]);
    const text = el.innerText.trim();
    const id = el.id || null;
    const selector = cssPath(el);
    let content = '';
    let sibling = el.nextElementSibling;
    while (sibling) {
      const sibTag = sibling.tagName.toLowerCase();
      if (/^h[1-6]$/.test(sibTag) && parseInt(sibTag[1]) <= level) break;
      content += sibling.innerText || '';
      sibling = sibling.nextElementSibling;
    }
    const char_count = content.length;
    const preview = char_count > 100 ? content.slice(0, 100).trim() + '...' : content.trim();
    return { level, text, id, selector, char_count, preview };
  });

  const landmarks = Array.from(root.querySelectorAll('nav, main, aside, article, footer, header, section[aria-label], [role="navigation"], [role="main"], [role="complementary"]')).map((el) => ({
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute('role'),
    id: el.id || null,
    selector: cssPath(el),
    text_preview: (el.innerText || '').trim().slice(0, 120),
  }));
  const regionSelector = 'nav, main, aside, article, header, footer, section, form, ' +
    '[role="dialog"], [role="alertdialog"], [role="region"], ' +
    '[role="navigation"], [role="main"], [role="complementary"], ' +
    '[role="banner"], [role="form"], ' +
    '[aria-modal="true"], section[aria-label], [popover]';
  const regionNodes = Array.from(root.querySelectorAll(regionSelector));
  const regionCandidates = regionNodes.map((el) => {
    const parent = el.parentElement;
    const parentIdx = parent ? regionNodes.indexOf(parent) : null;
    return {
      tag: el.tagName.toLowerCase(),
      role: el.getAttribute('role') || null,
      aria_label: el.getAttribute('aria-label') || null,
      id: el.id || null,
      depth: (function countDepth(node, scopeRoot) {
        let d = 0;
        let cur = node;
        while (cur && cur !== scopeRoot) {
          d++;
          cur = cur.parentElement;
        }
        return d;
      })(el, root),
      parent_idx: parentIdx >= 0 ? parentIdx : null,
      selector: cssPath(el),
      visible: !el.hidden && (function(e) { const s = window.getComputedStyle(e); return s.display !== 'none' && s.visibility !== 'hidden' && e.getBoundingClientRect().height > 0; })(el),
    };
  });
  const total_landmarks = landmarks.length;
  const cappedLandmarks = landmarks.slice(0, 20);

  const MAX_FORMS = 10;
  const MAX_FIELDS_PER_FORM = 30;
  const allForms = Array.from(root.querySelectorAll('form'));
  const total_forms = allForms.length;
  const forms = allForms.slice(0, MAX_FORMS).map((f) => {
    const allFields = Array.from(f.querySelectorAll('input, select, textarea'));
    const fields = allFields.slice(0, MAX_FIELDS_PER_FORM).map((el) => {
      const label = el.id
        ? document.querySelector(`label[for="${CSS.escape(el.id)}"]`)?.textContent?.trim() || el.placeholder || ''
        : el.placeholder || '';
      return {
        name: el.name || '',
        id: el.id || '',
        type: el.type || el.tagName.toLowerCase(),
        label,
        required: Boolean(el.required),
      };
    });
    return {
      action: f.action || '',
      method: f.method || 'get',
      id: f.id || null,
      selector: cssPath(f),
      fields,
      total_fields: allFields.length,
    };
  });

  const seenHrefs = new Set();
  const MAX_LINKS = 50;
  let total_links = 0;
  const links = [];
  for (const a of root.querySelectorAll('a[href]')) {
    const text = (a.textContent || '').trim();
    const rawHref = a.getAttribute('href') || '';
    const href = a.href || rawHref;
    if (!text || rawHref.startsWith('#') || seenHrefs.has(href)) continue;
    seenHrefs.add(href);
    total_links++;
    if (links.length < MAX_LINKS) {
      links.push({ text, href, selector: cssPath(a) });
    }
  }

  const MAX_INTERACTIVE = 30;
  const interactiveEls = [];
  const selectors = [
    ['button', 'button'],
    ['input', 'input:not([type="hidden"])'],
    ['select', 'select'],
    ['textarea', 'textarea'],
    ['a[role="button"]', 'a[role="button"]'],
    ['[role="tab"]', '[role="tab"]'],
    ['[role="menuitem"]', '[role="menuitem"]'],
    ['[role="option"]', '[role="option"]'],
    ['[role="switch"]', '[role="switch"]'],
    ['[role="checkbox"]', '[role="checkbox"]'],
  ];
  const counts = { buttons: 0, inputs: 0, selects: 0, textareas: 0, total: 0 };
  for (const [label, sel] of selectors) {
    for (const el of root.querySelectorAll(sel)) {
      counts.total++;
      if (el.tagName === 'BUTTON' || el.getAttribute('role') === 'button') counts.buttons++;
      else if (el.tagName === 'INPUT') counts.inputs++;
      else if (el.tagName === 'SELECT') counts.selects++;
      else if (el.tagName === 'TEXTAREA') counts.textareas++;
      if (interactiveEls.length < MAX_INTERACTIVE) {
        const entry = {
          tag: el.tagName.toLowerCase(),
          text: (el.textContent || el.value || '').trim().slice(0, 60),
          selector: cssPath(el),
        };
        if (el.disabled) entry.disabled = true;
        if (el.type) entry.type = el.type;
        if ((el.tagName === 'SELECT' || el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && el.value) {
          let val = el.value;
          if (el.tagName === 'SELECT' && el.selectedOptions && el.selectedOptions.length) {
            val = el.selectedOptions[0].text || val;
          }
          entry.value = val.slice(0, 60);
        }
        const ariaPressed = el.getAttribute('aria-pressed');
        if (ariaPressed) entry.aria_pressed = ariaPressed;
        const ariaExpanded = el.getAttribute('aria-expanded');
        if (ariaExpanded) entry.aria_expanded = ariaExpanded;
        const ariaSelected = el.getAttribute('aria-selected');
        if (ariaSelected) entry.aria_selected = ariaSelected;
        if (el.checked) entry.checked = true;

        // Computed ARIA role (always set, based on element type)
        const ariaRoleAttr = el.getAttribute('role');
        if (ariaRoleAttr) {
          entry.role = ariaRoleAttr;
        } else {
          const tag = el.tagName;
          const inputType = (el.type || '').toLowerCase();
          if (tag === 'BUTTON' || (tag === 'INPUT' && inputType === 'button') || (tag === 'INPUT' && inputType === 'submit') || (tag === 'INPUT' && inputType === 'reset') || (tag === 'INPUT' && inputType === 'image')) {
            entry.role = 'button';
          } else if (tag === 'A') {
            entry.role = 'link';
          } else if (tag === 'INPUT' && (inputType === 'checkbox')) {
            entry.role = 'checkbox';
          } else if (tag === 'INPUT' && (inputType === 'radio')) {
            entry.role = 'radio';
          } else if (tag === 'INPUT' && (inputType === 'range')) {
            entry.role = 'slider';
          } else if (tag === 'INPUT') {
            entry.role = 'textbox';
          } else if (tag === 'SELECT') {
            entry.role = 'combobox';
          } else if (tag === 'TEXTAREA') {
            entry.role = 'textbox';
          } else {
            entry.role = tag.toLowerCase();
          }
        }

        // Accessible name: prefer aria-label > aria-labelledby > innerText > placeholder > title > name attr
        const ariaLabel = el.getAttribute('aria-label');
        const ariaLabelledBy = el.getAttribute('aria-labelledby');
        let elName = '';
        if (ariaLabel) {
          elName = ariaLabel.trim().slice(0, 60);
        } else if (ariaLabelledBy) {
          const labelEl = document.getElementById(ariaLabelledBy);
          if (labelEl) elName = (labelEl.innerText || '').trim().slice(0, 60);
        }
        if (!elName) {
          const innerTxt = (el.innerText || '').trim();
          if (innerTxt) elName = innerTxt.slice(0, 60);
        }
        if (!elName && el.placeholder) elName = el.placeholder.slice(0, 60);
        if (!elName && el.title) elName = el.title.slice(0, 60);
        if (!elName && el.name) elName = el.name.slice(0, 60);
        if (elName) entry.name = elName;

        interactiveEls.push(entry);
      }
    }
  }
  const interactive = { counts, elements: interactiveEls };
  const nonFormControls = Array.from(root.querySelectorAll(
    'input:not(form input), select:not(form select), textarea:not(form textarea), ' +
    'button:not(form button), [role="checkbox"]:not(form [role="checkbox"]), ' +
    '[role="switch"]:not(form [role="switch"]), [role="combobox"]:not(form [role="combobox"])'
  )).map(el => ({
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute('role') || null,
    text: (el.innerText || '').trim().slice(0, 120),
    aria_label: el.getAttribute('aria-label') || null,
    aria_labelledby_text: (() => {
      const id = el.getAttribute('aria-labelledby');
      return id ? (document.getElementById(id)?.innerText?.trim() || null) : null;
    })(),
    title: el.title || null,
    placeholder: el.placeholder || null,
    name: el.name || null,
    value: el.value || null,
    required: Boolean(el.required),
    disabled: Boolean(el.disabled),
    selector: cssPath(el),
  }));

  const meta = {
    title: document.title,
    description: document.querySelector('meta[name="description"]')?.content || '',
    url: window.location.href,
  };

  return { headings, landmarks: cappedLandmarks, forms, links, interactive, meta, regionCandidates, nonFormControls, total_landmarks, total_forms, total_links };
}

async function handlePageMap(tabId, payload) {
  await ensureAttached(tabId);

  const scope = (payload && payload.scope) || null;
  const expression = scope
    ? `(${pageMapScript.toString()})(${JSON.stringify(scope)})`
    : `(${pageMapScript.toString()})(null)`;

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'page_map script threw exception');
  }

  return res.result?.value || {};
}
