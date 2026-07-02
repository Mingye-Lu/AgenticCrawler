'use strict';

// Uses the shared ARIA walk implementation (parity with CloakBrowser backend).

function pageMapScript(payload) {
const scope = payload?.scope || null;
const depth = Number.isFinite(payload?.depth) ? Math.min(Math.max(Math.floor(payload.depth), 1), 10) : 5;

function emptyResult(rawScope) {
  return {
    tree: {
      role: 'document',
      name: '',
      states: {},
      refId: null,
      url: null,
      frameId: null,
      offscreen: false,
      children: [],
      omittedChildren: 0,
    },
    url: window.location.href,
    meta: {
      title: document.title,
      description: document.querySelector('meta[name="description"]')?.content || '',
      url: window.location.href,
    },
    headings: [],
    landmarks: [],
    forms: [],
    links: [],
    interactive: { counts: { buttons: 0, inputs: 0, selects: 0, textareas: 0, total: 0 }, elements: [] },
    controls: [],
    regions: [],
    active_dialog: null,
    scope_not_found: false,
    scope: rawScope,
  };
}

function staleRefMessage(refId) {
  return "Ref '@" + refId + "' not found. The page may have changed. Call page_map to get fresh refs.";
}

// BEGIN_SHARED_ARIA_WALK
function getView(node) {
  return node && node.ownerDocument && node.ownerDocument.defaultView
    ? node.ownerDocument.defaultView
    : window;
}

function getComputed(node) {
  try {
    return getView(node).getComputedStyle(node);
  } catch (_) {
    return { display: '', visibility: '' };
  }
}

function isActuallyHidden(node) {
  const style = getComputed(node);
  return style.display === 'none'
    || style.visibility === 'hidden'
    || node.getAttribute('aria-hidden') === 'true'
    || node.hidden;
}

function isOffscreen(node) {
  try {
    const rect = node.getBoundingClientRect();
    const view = getView(node);
    return rect.bottom < 0
      || rect.right < 0
      || rect.top > view.innerHeight
      || rect.left > view.innerWidth;
  } catch (_) {
    return false;
  }
}

function getRole(el) {
  const explicitRole = (el.getAttribute('role') || '').trim();
  if (explicitRole) return explicitRole;
  const tag = el.tagName.toLowerCase();
  const type = (el.getAttribute('type') || '').toLowerCase();
  if (tag === 'button') return 'button';
  if (tag === 'a' && el.hasAttribute('href')) return 'link';
  if (tag === 'a') return 'generic';
  if (tag === 'input') {
    if (type === 'checkbox') return 'checkbox';
    if (type === 'radio') return 'radio';
    if (type === 'submit' || type === 'button' || type === 'reset' || type === 'image') return 'button';
    if (type === 'range') return 'slider';
    return 'textbox';
  }
  if (tag === 'select') return 'combobox';
  if (tag === 'textarea') return 'textbox';
  if (/^h[1-6]$/.test(tag)) return 'heading';
  if (tag === 'nav') return 'navigation';
  if (tag === 'main') return 'main';
  if (tag === 'header') return el.closest('article,aside,nav,section') ? 'generic' : 'banner';
  if (tag === 'footer') return el.closest('article,aside,nav,section') ? 'generic' : 'contentinfo';
  if (tag === 'form') {
    return el.getAttribute('aria-label') || el.getAttribute('aria-labelledby') || el.getAttribute('name')
      ? 'form'
      : 'generic';
  }
  if (tag === 'section') return el.getAttribute('aria-label') || el.getAttribute('aria-labelledby') ? 'region' : 'generic';
  if (tag === 'aside') return 'complementary';
  if (tag === 'search') return 'search';
  if (tag === 'dialog') return 'dialog';
  if (tag === 'article') return 'article';
  if (tag === 'ul' || tag === 'ol') return 'list';
  if (tag === 'li') return 'listitem';
  if (tag === 'table') return 'table';
  if (tag === 'tr') return 'row';
  if (tag === 'th') return el.getAttribute('scope') === 'row' ? 'rowheader' : 'columnheader';
  if (tag === 'td') return 'cell';
  if (tag === 'img') return el.getAttribute('alt') === '' ? 'presentation' : 'img';
  if (tag === 'iframe') return 'iframe';
  return 'generic';
}

function resolveAriaLabelledbyText(el) {
  const ids = (el.getAttribute('aria-labelledby') || '').trim();
  if (!ids) return '';
  const doc = el.ownerDocument || document;
  return ids
    .split(/\s+/)
    .map((id) => doc.getElementById(id))
    .filter(Boolean)
    .map((node) => (node.innerText || node.textContent || '').trim())
    .filter(Boolean)
    .join(' ')
    .trim();
}

function resolveLabelText(el) {
  const doc = el.ownerDocument || document;
  if (el.id) {
    const explicit = doc.querySelector('label[for="' + CSS.escape(el.id) + '"]');
    const explicitText = explicit ? (explicit.innerText || explicit.textContent || '').trim() : '';
    if (explicitText) return explicitText;
  }
  const wrapped = el.closest('label');
  return wrapped ? (wrapped.innerText || wrapped.textContent || '').trim() : '';
}

function getAccessibleName(el, role) {
  const ariaLabelledByText = resolveAriaLabelledbyText(el);
  if (ariaLabelledByText) return ariaLabelledByText;

  const ariaLabel = (el.getAttribute('aria-label') || '').trim();
  if (ariaLabel) return ariaLabel;

  const labelText = resolveLabelText(el);
  if (labelText) return labelText;

  const innerText = (el.innerText || el.textContent || '').trim();
  if (innerText && (role === 'heading'
    || role === 'navigation'
    || role === 'main'
    || role === 'banner'
    || role === 'contentinfo'
    || role === 'complementary'
    || role === 'region'
    || role === 'form'
    || role === 'search'
    || role === 'dialog'
    || role === 'article'
    || role === 'button'
    || role === 'link'
    || role === 'tab'
    || role === 'menuitem'
    || role === 'option'
    || role === 'listitem')) {
    return innerText;
  }

  const title = (el.getAttribute('title') || '').trim();
  if (title) return title;

  const placeholder = (el.getAttribute('placeholder') || '').trim();
  if (placeholder) return placeholder;

  if (innerText) return innerText;

  const nameAttr = (el.getAttribute('name') || '').trim();
  if (nameAttr) return nameAttr;

  return '';
}

function getStates(el, role) {
  const states = {};
  if (el.disabled || el.getAttribute('aria-disabled') === 'true') states.disabled = true;
  if (el.checked || el.getAttribute('aria-checked') === 'true') states.checked = true;
  const expanded = el.getAttribute('aria-expanded');
  if (expanded !== null) states.expanded = expanded === 'true';
  const pressed = el.getAttribute('aria-pressed');
  if (pressed !== null) states.pressed = pressed === 'true';
  if (el.getAttribute('aria-selected') === 'true') states.selected = true;
  const ariaLevel = el.getAttribute('aria-level');
  if (role === 'heading') {
    const level = /^h[1-6]$/i.test(el.tagName) ? Number.parseInt(el.tagName[1], 10) : Number.parseInt(ariaLevel || '', 10);
    if (Number.isFinite(level) && level > 0) states.level = level;
  } else if (ariaLevel !== null) {
    const level = Number.parseInt(ariaLevel, 10);
    if (Number.isFinite(level) && level > 0) states.level = level;
  }
  const ownerDoc = el.ownerDocument || document;
  if (el.getAttribute('aria-current') === 'true' || el === ownerDoc.activeElement) states.active = true;
  if (el.getAttribute('aria-invalid') === 'true' || (el.validity && !el.validity.valid)) states.invalid = true;
  return states;
}

function hasStates(states) {
  return Object.keys(states).length > 0;
}

function isAlwaysIncluded(role) {
  return new Set([
    'button', 'link', 'textbox', 'checkbox', 'radio', 'combobox',
    'slider', 'switch', 'tab', 'menuitem', 'option', 'main',
    'navigation', 'banner', 'contentinfo', 'complementary', 'form',
    'search', 'heading', 'iframe', 'dialog', 'listitem'
  ]).has(role);
}

function isFocusable(el) {
  const tabindex = el.getAttribute('tabindex');
  return tabindex !== null || typeof el.tabIndex === 'number' && el.tabIndex >= 0;
}

function shouldInclude(el, role, name, states) {
  if (isActuallyHidden(el)) return false;
  if (isAlwaysIncluded(role)) return true;
  if (isFocusable(el)) return true;
  if (name && name.trim() !== '' && ['region', 'article', 'section', 'group', 'list', 'listitem'].includes(role)) return true;
  if (role === 'generic' && (!name || name.trim() === '') && !hasStates(states)) return false;
  return true;
}

function ensureStampedRef(el, refCounter) {
  const existing = (el.getAttribute('data-acrawl-ref') || '').trim();
  if (existing) return existing;
  let next;
  do {
    next = 'e' + (++refCounter.n);
  } while (refCounter.used.has(next));
  refCounter.used.add(next);
  el.setAttribute('data-acrawl-ref', next);
  return next;
}

function seedCountersFromRoot(rootEl, refCounter) {
  const stack = [rootEl];
  while (stack.length > 0) {
    const current = stack.pop();
    if (!current || current.nodeType !== 1) continue;
    const existing = (current.getAttribute('data-acrawl-ref') || '').trim();
    if (/^e\d+$/.test(existing)) {
      refCounter.used.add(existing);
      const numeric = Number.parseInt(existing.slice(1), 10);
      if (numeric > refCounter.n) refCounter.n = numeric;
    }
    if (current.tagName && current.tagName.toLowerCase() === 'iframe') {
      try {
        const frameDoc = current.contentDocument;
        if (frameDoc && frameDoc.body) stack.push(frameDoc.body);
      } catch (_) {}
    }
    for (const child of Array.from(current.children || [])) stack.push(child);
  }
}

function findActiveDialog() {
  const candidates = Array.from(document.querySelectorAll('[role="dialog"], [role="alertdialog"], dialog, [aria-modal="true"], [popover]'));
  for (const candidate of candidates) {
    if (!isActuallyHidden(candidate)) return candidate;
  }
  return null;
}

function findStampedElementByRef(refId, docRoot) {
  const found = docRoot.querySelector('[data-acrawl-ref="' + CSS.escape(refId) + '"]');
  if (found) return found;
  for (const iframe of Array.from(docRoot.querySelectorAll('iframe'))) {
    try {
      const frameDoc = iframe.contentDocument;
      if (!frameDoc) continue;
      const nested = findStampedElementByRef(refId, frameDoc);
      if (nested) return nested;
    } catch (_) {}
  }
  return null;
}

function findScopedElement(rawScope, docRoot) {
  let found = null;
  try {
    found = docRoot.querySelector(rawScope);
  } catch (_) {
    return null;
  }
  if (found) return found;
  for (const iframe of Array.from(docRoot.querySelectorAll('iframe'))) {
    try {
      const frameDoc = iframe.contentDocument;
      if (!frameDoc) continue;
      const nested = findScopedElement(rawScope, frameDoc);
      if (nested) return nested;
    } catch (_) {}
  }
  return null;
}

function resolveScopeRoot(rawScope) {
  if (!rawScope) return { root: document.body || document.documentElement, kind: 'ok' };
  if (rawScope === 'dialog') {
    const dialogRoot = findActiveDialog();
    return dialogRoot ? { root: dialogRoot, kind: 'ok' } : { root: null, kind: 'scope_not_found' };
  }
  if (rawScope === 'main') {
    const mainRoot = document.querySelector('main, [role="main"]');
    return mainRoot ? { root: mainRoot, kind: 'ok' } : { root: null, kind: 'scope_not_found' };
  }
  if (rawScope === 'sidebar') {
    const sidebarRoot = document.querySelector('[role="complementary"], aside, nav');
    return sidebarRoot ? { root: sidebarRoot, kind: 'ok' } : { root: null, kind: 'scope_not_found' };
  }
  const refMatch = rawScope.match(/^\[ref=(e\d+)\]$/)
    || rawScope.match(/^@?(e\d+)$/)
    || rawScope.match(/^\[data-acrawl-ref=['"]?(e\d+)['"]?\]$/);
  if (refMatch) {
    const refId = refMatch[1];
    const refRoot = findStampedElementByRef(refId, document);
    return refRoot
      ? { root: refRoot, kind: 'ok' }
      : { root: null, kind: 'stale_ref', refId };
  }
  const scoped = findScopedElement(rawScope, document);
  return scoped ? { root: scoped, kind: 'ok' } : { root: null, kind: 'scope_not_found' };
}

function createNode(role, name, states, refId, frameId, el) {
  return {
    role,
    name: name || '',
    states,
    refId,
    url: role === 'link' ? (el.href || el.getAttribute('href') || null) : null,
    frameId: frameId || null,
    offscreen: isOffscreen(el),
    children: [],
    omittedChildren: 0,
  };
}

function countRetainedChildren(el) {
  let count = 0;
  for (const child of Array.from(el.children || [])) {
    if (child.nodeType !== 1) continue;
    const role = getRole(child);
    const name = getAccessibleName(child, role);
    const states = getStates(child, role);
    if (!shouldInclude(child, role, name, states)) {
      if (role === 'generic') count += countRetainedChildren(child);
      continue;
    }
    count += 1;
  }
  return count;
}

function walkChildren(rootEl, ctx, frameId, depthLevel) {
  const retained = [];
  for (const child of Array.from(rootEl.children || [])) {
    if (ctx.totalNodes.overflow) break;
    retained.push(...ariaWalk(child, ctx, frameId, depthLevel));
  }
  return retained;
}

function walkIframe(iframeEl, ctx, depthLevel) {
  try {
    const frameDoc = iframeEl.contentDocument;
    if (!frameDoc || !frameDoc.body) {
      return { frameId: null, children: [], crossOrigin: false };
    }
    const frameId = 'f' + (++ctx.refCounter.frameN);
    seedCountersFromRoot(frameDoc.body, ctx.refCounter);
    return {
      frameId,
      children: walkChildren(frameDoc.body, ctx, frameId, depthLevel + 1),
      crossOrigin: false,
    };
  } catch (_) {
    return { frameId: null, children: [], crossOrigin: true };
  }
}

function ariaWalk(el, ctx, frameId, depthLevel) {
  if (!el || el.nodeType !== 1 || ctx.totalNodes.overflow) return [];

  const role = getRole(el);
  const name = getAccessibleName(el, role);
  const states = getStates(el, role);

  if (!shouldInclude(el, role, name, states)) {
    if (role === 'generic') {
      return walkChildren(el, ctx, frameId, depthLevel + 1);
    }
    return [];
  }

  ctx.totalNodes.count += 1;
  if (ctx.totalNodes.count > 2000) {
    ctx.totalNodes.overflow = true;
    return [];
  }

  const refId = ensureStampedRef(el, ctx.refCounter);
  const node = createNode(role, name, states, refId, frameId, el);

  if (role === 'iframe') {
    const iframeResult = walkIframe(el, ctx, depthLevel);
    if (iframeResult.crossOrigin) node.crossOrigin = true;
    if (iframeResult.frameId) node.frameId = iframeResult.frameId;
    node.children = iframeResult.children;
    return [node];
  }

  // future: pierce open shadow roots (see PR note)

  if (ctx.degraded && depthLevel >= 1) {
    node.omittedChildren = countRetainedChildren(el);
    return [node];
  }

  if (depthLevel >= ctx.maxDepth - 1) {
    node.omittedChildren = countRetainedChildren(el);
    return [node];
  }

  const remainingSlots = () => Math.max(0, 50 - node.children.length);
  for (const child of Array.from(el.children || [])) {
    if (ctx.totalNodes.overflow) break;
    const childNodes = ariaWalk(child, ctx, frameId, depthLevel + 1);
    if (childNodes.length === 0) continue;
    const slots = remainingSlots();
    if (slots <= 0) {
      node.omittedChildren += childNodes.length;
      continue;
    }
    if (childNodes.length <= slots) {
      node.children.push(...childNodes);
    } else {
      node.children.push(...childNodes.slice(0, slots));
      node.omittedChildren += childNodes.length - slots;
    }
  }

  return [node];
}

function buildTree(rootEl, maxDepth, degraded) {
  const ctx = {
    maxDepth,
    degraded,
    refCounter: { n: 0, frameN: 0, used: new Set() },
    totalNodes: { count: 0, overflow: false },
  };
  seedCountersFromRoot(rootEl, ctx.refCounter);
  const wrapper = {
    role: 'document',
    name: '',
    states: {},
    refId: null,
    url: null,
    frameId: null,
    offscreen: false,
    children: ariaWalk(rootEl, ctx, null, 0),
    omittedChildren: 0,
  };
  return { wrapper, overflow: ctx.totalNodes.overflow };
}
// END_SHARED_ARIA_WALK

const resolvedScope = resolveScopeRoot(scope);
if (resolvedScope.kind === 'stale_ref') {
  const stale = emptyResult(scope);
  stale.stale_ref = true;
  stale.error = staleRefMessage(resolvedScope.refId);
  return stale;
}
if (resolvedScope.kind !== 'ok' || !resolvedScope.root) {
  const empty = emptyResult(scope);
  empty.scope_not_found = true;
  return empty;
}

const firstPass = buildTree(resolvedScope.root, depth, false);
const tree = firstPass.overflow ? buildTree(resolvedScope.root, 2, true).wrapper : firstPass.wrapper;
const result = emptyResult(scope);
result.tree = tree;
return result;
}

async function handlePageMap(tabId, payload) {
  await ensureAttached(tabId);

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: `(${pageMapScript.toString()})(${JSON.stringify(payload || {})})`,
    returnByValue: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'page_map script threw exception');
  }

  return res.result?.value || {};
}
