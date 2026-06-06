'use strict';

const pageMapCache = new Map();

function urlWithoutHash(url) {
  const idx = url.indexOf('#');
  if (idx === -1) return url;
  const frag = url.slice(idx + 1);
  if (frag.startsWith('/') || frag.startsWith('!/')) return url;
  return url.slice(0, idx);
}

function headingKey(h) { return `${h.level}:${h.text}`; }
function linkKey(l) { return `${l.text}\x00${l.href}`; }
function landmarkKey(lm) { return `${lm.tag || ''}\x00${lm.role || ''}\x00${lm.id || ''}`; }

function diffArrays(prev, curr, keyFn) {
  const prevCounts = Object.create(null);
  for (const item of prev) {
    const k = keyFn(item);
    prevCounts[k] = (prevCounts[k] || 0) + 1;
  }
  const currCounts = Object.create(null);
  for (const item of curr) {
    const k = keyFn(item);
    currCounts[k] = (currCounts[k] || 0) + 1;
  }

  const addedBudget = Object.create(null);
  for (const k of Object.keys(currCounts)) {
    const diff = currCounts[k] - (prevCounts[k] || 0);
    if (diff > 0) addedBudget[k] = diff;
  }
  const removedBudget = Object.create(null);
  for (const k of Object.keys(prevCounts)) {
    const diff = prevCounts[k] - (currCounts[k] || 0);
    if (diff > 0) removedBudget[k] = diff;
  }

  const added = curr.filter(item => {
    const k = keyFn(item);
    if (addedBudget[k] > 0) { addedBudget[k]--; return true; }
    return false;
  });
  const removed = prev.filter(item => {
    const k = keyFn(item);
    if (removedBudget[k] > 0) { removedBudget[k]--; return true; }
    return false;
  });
  return { added, removed };
}

function computePageMapDiff(prev, current) {
  const url = current.meta?.url || 'unknown';
  const title = current.meta?.title || 'unknown';

  const { added: addedHeadings, removed: removedHeadings } =
    diffArrays(prev.headings || [], current.headings || [], headingKey);
  const { added: addedLinks, removed: removedLinks } =
    diffArrays(prev.links || [], current.links || [], linkKey);
  const { added: addedLandmarks, removed: removedLandmarks } =
    diffArrays(prev.landmarks || [], current.landmarks || [], landmarkKey);

  const STATE_FIELDS = ['disabled', 'checked', 'value', 'aria_pressed', 'aria_expanded', 'aria_selected'];
  const MAX_INTERACTIVE_DIFF = 5;
  const prevElements = prev.interactive?.elements || [];
  const currElements = current.interactive?.elements || [];
  const prevBySelector = Object.create(null);
  for (const el of prevElements) {
    if (el.selector) prevBySelector[el.selector] = el;
  }
  const currBySelector = Object.create(null);
  for (const el of currElements) {
    if (el.selector) currBySelector[el.selector] = el;
  }

  const briefEntry = (el) => {
    const e = { selector: el.selector };
    if (el.tag) e.tag = el.tag;
    if (el.text) e.text = el.text;
    if (el.role) e.role = el.role;
    return e;
  };

  const addedInteractive = currElements
    .filter(el => el.selector && !prevBySelector[el.selector])
    .slice(0, MAX_INTERACTIVE_DIFF)
    .map(briefEntry);

  const removedInteractive = prevElements
    .filter(el => el.selector && !currBySelector[el.selector])
    .slice(0, MAX_INTERACTIVE_DIFF)
    .map(briefEntry);

  const modifiedInteractive = [];
  for (const el of currElements) {
    if (!el.selector) continue;
    const prevEl = prevBySelector[el.selector];
    if (!prevEl) continue;
    const stateChanges = {};
    for (const field of STATE_FIELDS) {
      const pv = prevEl[field] ?? null;
      const cv = el[field] ?? null;
      if (pv !== cv) stateChanges[field] = cv;
    }
    if (Object.keys(stateChanges).length > 0) {
      const entry = { selector: el.selector };
      if (el.tag) entry.tag = el.tag;
      if (el.text) entry.text = el.text;
      entry.state_changes = stateChanges;
      modifiedInteractive.push(entry);
    }
  }

  const hasChanges = addedHeadings.length + removedHeadings.length +
    addedLinks.length + removedLinks.length +
    addedLandmarks.length + removedLandmarks.length +
    addedInteractive.length + removedInteractive.length +
    modifiedInteractive.length > 0;

  if (!hasChanges) {
    return { url, title, changed: false };
  }

  const totalPrev = (prev.headings || []).length +
    (prev.links || []).length + (prev.landmarks || []).length;
  const totalChanged = addedHeadings.length + removedHeadings.length +
    addedLinks.length + removedLinks.length +
    addedLandmarks.length + removedLandmarks.length;

  if (totalPrev > 0 && totalChanged > totalPrev) {
    return null; // diff too large, caller should return full
  }

  const changes = {};
  if (addedHeadings.length) changes.added_headings = addedHeadings;
  if (removedHeadings.length) changes.removed_headings = removedHeadings;
  if (addedLinks.length) changes.added_links = addedLinks;
  if (removedLinks.length) changes.removed_links = removedLinks;
  if (addedLandmarks.length) changes.added_landmarks = addedLandmarks;
  if (removedLandmarks.length) changes.removed_landmarks = removedLandmarks;
  if (addedInteractive.length) changes.added_interactive = addedInteractive;
  if (removedInteractive.length) changes.removed_interactive = removedInteractive;
  if (modifiedInteractive.length) changes.modified_interactive = modifiedInteractive;

  return { url, title, changed: true, changes };
}

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

  const meta = {
    title: document.title,
    description: document.querySelector('meta[name="description"]')?.content || '',
    url: window.location.href,
  };

  return { headings, landmarks: cappedLandmarks, forms, links, interactive, meta, total_landmarks, total_forms, total_links };
}

async function handlePageMap(tabId, payload) {
  await ensureAttached(tabId);

  const scope = (payload && payload.scope) || null;
  const diffMode = Boolean(payload && payload.diff);
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

  const current = res.result?.value || {};

  if (diffMode && !scope) {
    const currentUrl = urlWithoutHash(current.meta?.url || '');
    const cached = pageMapCache.get(tabId);

    if (cached && cached.url === currentUrl) {
      const diff = computePageMapDiff(cached.data, current);
      pageMapCache.set(tabId, { url: currentUrl, data: current });
      if (diff !== null) {
        return diff;
      }
    } else {
      pageMapCache.set(tabId, { url: currentUrl, data: current });
    }
  } else if (!scope) {
    const currentUrl = urlWithoutHash(current.meta?.url || '');
    pageMapCache.set(tabId, { url: currentUrl, data: current });
  }

  return current;
}
