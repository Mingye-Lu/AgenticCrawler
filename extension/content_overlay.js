'use strict';

// Visual feedback layer: a glowing viewport border while acrawl drives this tab,
// click ripples, and boxes over elements that changed after an action. Every
// element is non-interactive and lives in a closed shadow root. The service
// worker (commands/overlay.js) decides when to show things; this file only draws.
(() => {
  const RIPPLE_MS = 700;
  const BOX_MS = 3200;
  const FLASH_MS = 900;
  const MAX_BOXES = 40;
  const REF_RE = /^e\d+$/;

  const STYLE = `
    :host { all: initial; }
    * { box-sizing: border-box; pointer-events: none; }
    .border {
      position: fixed; inset: 0; opacity: 0; transition: opacity .25s ease;
      box-shadow: inset 0 0 0 3px rgba(99,102,241,.85), inset 0 0 28px 6px rgba(99,102,241,.35);
    }
    .border.on { opacity: 1; animation: pulse 2.4s ease-in-out infinite; }
    @keyframes pulse { 50% { box-shadow: inset 0 0 0 3px rgba(99,102,241,.6), inset 0 0 40px 10px rgba(99,102,241,.2); } }
    .ripple {
      position: fixed; width: 44px; height: 44px; margin: -22px 0 0 -22px; border-radius: 50%;
      border: 3px solid var(--c); background: color-mix(in srgb, var(--c) 22%, transparent);
      animation: ripple ${RIPPLE_MS}ms ease-out forwards;
    }
    .ripple.click { --c: #3b82f6; }
    .ripple.hover { --c: #94a3b8; width: 26px; height: 26px; margin: -13px 0 0 -13px; }
    @keyframes ripple { from { transform: scale(.2); opacity: 1; } to { transform: scale(1.3); opacity: 0; } }
    .box {
      position: fixed; border: 2px solid var(--c); border-radius: 6px;
      background: color-mix(in srgb, var(--c) 12%, transparent);
      box-shadow: 0 0 12px 1px color-mix(in srgb, var(--c) 45%, transparent);
      animation: boxfade ${BOX_MS}ms ease-out forwards, boxpop 350ms ease-out;
    }
    .box.added { --c: #22c55e; }
    .box.changed { --c: #3b82f6; }
    .box.fill { --c: #3b82f6; animation-duration: ${FLASH_MS}ms, 350ms; }
    @keyframes boxpop { from { transform: scale(1.06); } to { transform: scale(1); } }
    @keyframes boxfade { 0%, 70% { opacity: 1; } 100% { opacity: 0; } }
    @media (prefers-reduced-motion: reduce) {
      .border.on { animation: none; }
      @keyframes ripple { from { opacity: 1; } to { opacity: 0; } }
    }
  `;

  let layer = null;
  let borderEl = null;
  let borderTimer = null;
  let suspended = false;
  let boxes = []; // { div, el, frames }
  let rafId = 0;

  function ensureLayer() {
    if (layer && layer.host.isConnected) return layer;
    const host = document.createElement('div');
    host.setAttribute('data-acrawl-overlay', '');
    host.style.cssText = 'all:initial;position:fixed;inset:0;pointer-events:none;z-index:2147483647;'
      + (suspended ? 'visibility:hidden;' : '');
    layer = host.attachShadow({ mode: 'closed' });
    const style = document.createElement('style');
    style.textContent = STYLE;
    borderEl = document.createElement('div');
    borderEl.className = 'border';
    layer.append(style, borderEl);
    document.documentElement.appendChild(host);
    return layer;
  }

  function add(className, styles) {
    const div = document.createElement('div');
    div.className = className;
    Object.assign(div.style, styles);
    ensureLayer().appendChild(div);
    return div;
  }

  function setActive(ttl) {
    ensureLayer();
    borderEl.classList.add('on');
    clearTimeout(borderTimer);
    borderTimer = setTimeout(() => borderEl && borderEl.classList.remove('on'), ttl);
  }

  function hideBorder() {
    clearTimeout(borderTimer);
    if (borderEl) borderEl.classList.remove('on');
  }

  function ripple(x, y, kind) {
    const div = add('ripple ' + (kind === 'hover' ? 'hover' : 'click'), { left: x + 'px', top: y + 'px' });
    setTimeout(() => div.remove(), RIPPLE_MS + 100);
  }

  function flash(rect) {
    const div = add('box fill', boxStyle(rect.x, rect.y, rect.width, rect.height));
    setTimeout(() => div.remove(), FLASH_MS + 100);
  }

  function boxStyle(x, y, w, h) {
    return { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' };
  }

  // Finds a stamped element, descending into same-origin iframes; `frames` is
  // the chain of iframe elements crossed, used to translate rects to top-level.
  function findRef(ref, doc, frames) {
    const el = doc.querySelector('[data-acrawl-ref="' + CSS.escape(ref) + '"]');
    if (el) return { el, frames };
    for (const iframe of doc.querySelectorAll('iframe')) {
      try {
        if (!iframe.contentDocument) continue;
        const hit = findRef(ref, iframe.contentDocument, frames.concat(iframe));
        if (hit) return hit;
      } catch (_) { /* cross-origin */ }
    }
    return null;
  }

  function topRect(el, frames) {
    const r = el.getBoundingClientRect();
    let x = r.left;
    let y = r.top;
    for (const f of frames) {
      const fr = f.getBoundingClientRect();
      x += fr.left + f.clientLeft;
      y += fr.top + f.clientTop;
    }
    return { x, y, width: r.width, height: r.height };
  }

  function clearBoxes() {
    cancelAnimationFrame(rafId);
    rafId = 0;
    for (const b of boxes) b.div.remove();
    boxes = [];
  }

  function place(box) {
    // visibility, not display: toggling display restarts the CSS animation (scrolling a box off and back on would replay the intro).
    if (!box.el.isConnected) {
      box.div.style.visibility = 'hidden';
      return;
    }
    const r = topRect(box.el, box.frames);
    const visible = r.width > 0 && r.height > 0
      && r.x < innerWidth && r.y < innerHeight && r.x + r.width > 0 && r.y + r.height > 0;
    box.div.style.visibility = visible ? 'visible' : 'hidden';
    Object.assign(box.div.style, boxStyle(r.x, r.y, r.width, r.height));
  }

  // Draws boxes over [el, frames, kind] jobs and keeps them glued to their
  // elements while the page scrolls or reflows. Each box owns its lifetime, so
  // appending new boxes never restarts the animation of older ones.
  function showBoxes(jobs, append = false) {
    if (!append) clearBoxes();
    for (const [el, frames, kind] of jobs) {
      if (boxes.length >= MAX_BOXES) break;
      const box = { div: add('box ' + kind, {}), el, frames };
      place(box);
      boxes.push(box);
      // Timer-driven: rAF is throttled in background tabs.
      setTimeout(() => {
        box.div.remove();
        boxes = boxes.filter((b) => b !== box);
      }, BOX_MS + 100);
    }
    if (boxes.length && !rafId) {
      const tick = () => {
        boxes.forEach(place);
        rafId = boxes.length ? requestAnimationFrame(tick) : 0;
      };
      rafId = requestAnimationFrame(tick);
    }
  }

  // A box over the whole page is noise: the diff root (BODY/HTML) is never a real change.
  const isPageRoot = (el) => el === el.ownerDocument.body || el === el.ownerDocument.documentElement;

  function highlight(added, changed) {
    const jobs = [
      ...added.map((ref) => [ref, 'added']),
      ...changed.map((ref) => [ref, 'changed']),
    ].flatMap(([ref, kind]) => {
      const hit = REF_RE.test(ref) && findRef(ref, document, []);
      return hit && !isPageRoot(hit.el) ? [[hit.el, hit.frames, kind]] : [];
    });
    showBoxes(jobs);
  }

  // Frontend-only hint of what an action added, independent of the agent's
  // (Rust-side) page diff: watch the DOM for a moment after each click/hover/fill
  // and box the elements that appeared.
  const WATCH_MS = 1500;
  // State attributes whose change counts as "modified" (blue); `open`/`hidden` mean "revealed" (green).
  const CHANGE_ATTRS = ['aria-expanded', 'aria-checked', 'aria-selected', 'aria-pressed', 'aria-current', 'disabled'];
  const IGNORED_TAGS = new Set(['SCRIPT', 'STYLE', 'LINK', 'META', 'NOSCRIPT', 'TEMPLATE']);
  let highlightTab = null; // cached: is change highlighting on? (null = unknown)
  let managedTab = null; // cached worker answer: is this tab in the acrawl group? (null = unknown)
  let lastAgentRipple = 0; // the agent's own CDP clicks are trusted events too; don't double-draw them
  let watcher = null; // { observer, added: Set<Element>, frame, stop }

  const boxedUntil = new WeakMap(); // element -> when its box expires; dedupes within a box's life only
  const isBoxed = (el) => (boxedUntil.get(el) || 0) > Date.now();

  const isShown = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };

  // A wrapper can be zero-sized (e.g. a portal root whose child is position:fixed),
  // so look through it to its visible descendants.
  const isPageSized = (el) => {
    const r = el.getBoundingClientRect();
    return r.width * r.height >= innerWidth * innerHeight * 0.6;
  };

  // Page-sized wrappers (a modal backdrop or portal root) are looked through too:
  // the useful target is the smaller dialog inside.
  function visibleTops(el) {
    if (!el.isConnected || IGNORED_TAGS.has(el.tagName) || el.hasAttribute('data-acrawl-overlay')) return [];
    return isShown(el) && !isPageSized(el) ? [el] : [...el.children].flatMap(visibleTops);
  }

  function collectAdded(w) {
    const cands = new Set([...w.added].flatMap(visibleTops));
    const tops = [...cands].filter((el) => {
      for (let p = el.parentElement; p; p = p.parentElement) if (cands.has(p)) return false;
      return !isPageRoot(el);
    });
    const fresh = tops.filter((el) => !isBoxed(el));
    // Modified (text/state changed) elements, unless they sit inside something newly added.
    const boxedAdded = new Set(tops);
    const inAdded = (el) => {
      for (let p = el; p; p = p.parentElement) if (boxedAdded.has(p)) return true;
      return false;
    };
    const modified = [...w.changed].filter((el) => el.isConnected && !isBoxed(el) && !inAdded(el)
      && isShown(el) && !isPageSized(el) && !isPageRoot(el));
    fresh.concat(modified).forEach((el) => boxedUntil.set(el, Date.now() + BOX_MS));
    showBoxes([
      ...fresh.map((el) => [el, [], 'added']),
      ...modified.map((el) => [el, [], 'changed']),
    ], true);
  }

  function stopWatching() {
    if (!watcher) return;
    const w = watcher;
    watcher = null;
    w.observer.disconnect();
    cancelAnimationFrame(w.frame);
    clearTimeout(w.stop);
    if (!w.gated) collectAdded(w); // boxedEls dedupes; catches elements that had no layout on their first frame
  }

  function cancelWatch(w) {
    if (watcher !== w) return;
    watcher = null;
    w.observer.disconnect();
    cancelAnimationFrame(w.frame);
    clearTimeout(w.stop);
  }

  function watchAdditions() {
    if (watcher) {
      clearTimeout(watcher.stop);
      watcher.stop = setTimeout(stopWatching, WATCH_MS);
      return watcher;
    }
    const w = { added: new Set(), changed: new Set(), gated: false, frame: 0, stop: setTimeout(stopWatching, WATCH_MS), observer: null };
    w.observer = new MutationObserver((records) => {
      for (const rec of records) {
        const t = rec.target;
        for (const n of rec.addedNodes) {
          if (n.nodeType === 1) w.added.add(n);
          else if (n.nodeType === 3 && n.nodeValue.trim() && t.nodeType === 1) w.changed.add(t); // textContent = "…"
        }
        if (rec.type === 'characterData') {
          if (t.parentElement) w.changed.add(t.parentElement);
        } else if (rec.type === 'attributes') {
          const name = rec.attributeName;
          // Dialogs and menus are often already in the DOM and merely get revealed.
          if (name === 'open') {
            if (rec.oldValue === null && t.hasAttribute('open')) w.added.add(t);
          } else if (name === 'hidden') {
            if (rec.oldValue !== null && !t.hasAttribute('hidden')) w.added.add(t);
          } else if (rec.oldValue !== t.getAttribute(name)) {
            w.changed.add(t);
          }
        }
      }
      // Draw on the next frame, not after the DOM settles, so the box lands with the element.
      if (!w.frame) {
        w.frame = requestAnimationFrame(() => {
          w.frame = 0;
          if (!w.gated) collectAdded(w);
        });
      }
    });
    w.observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
      attributes: true,
      characterData: true,
      attributeOldValue: true,
      attributeFilter: ['open', 'hidden', ...CHANGE_ATTRS],
    });
    watcher = w;
    return w;
  }

  // Human clicks in an acrawl-group tab get the same treatment as the agent's.
  // The listener is registered everywhere; the worker says whether this tab is
  // managed. Watching starts immediately (the DOM reacts within ms) and is
  // dropped if the tab turns out not to be ours.
  document.addEventListener('click', (e) => {
    if (!e.isTrusted || Date.now() - lastAgentRipple < 600) return;
    const target = e.target instanceof Element ? e.target : null;
    const rect = target && target.getBoundingClientRect();
    const x = e.clientX || e.clientY ? e.clientX : rect ? rect.left + rect.width / 2 : 0;
    const y = e.clientX || e.clientY ? e.clientY : rect ? rect.top + rect.height / 2 : 0;
    // Known-managed tabs react instantly; otherwise hold the draw until the worker answers.
    const w = watchAdditions();
    w.gated = managedTab !== true || highlightTab !== true;
    const rippled = managedTab === true;
    if (rippled) ripple(x, y, 'click');
    chrome.runtime.sendMessage({ type: 'acrawl_overlay_hello' }, (reply) => {
      void chrome.runtime.lastError;
      managedTab = !!(reply && reply.managed);
      highlightTab = !!(reply && reply.highlight);
      if (!managedTab || !highlightTab) {
        cancelWatch(w);
        clearBoxes();
        if (managedTab && !rippled) ripple(x, y, 'click');
        return;
      }
      if (w.gated) {
        w.gated = false;
        if (!rippled) ripple(x, y, 'click');
        collectAdded(w);
      }
    });
  }, true);

  chrome.runtime.onMessage.addListener((msg, _sender, respond) => {
    if (!msg || msg.type !== 'acrawl_overlay') return;
    managedTab = true;
    switch (msg.op) {
      case 'active': setActive(msg.ttl); break;
      case 'hide': hideBorder(); clearBoxes(); break;
      case 'watch': watchAdditions(); break;
      case 'ripple': ripple(msg.x, msg.y, msg.kind); lastAgentRipple = Date.now(); break;
      case 'flash': flash(msg.rect); break;
      case 'highlight': highlight(msg.added || [], msg.changed || []); break;
      case 'suspend':
        suspended = !!msg.on;
        if (layer) layer.host.style.visibility = suspended ? 'hidden' : 'visible';
        break;
      default: break;
    }
    respond({ ok: true });
  });

  // A navigation destroys this script's state; ask the worker whether the
  // border should still be showing on the fresh page.
  chrome.runtime.sendMessage({ type: 'acrawl_overlay_hello' }, (reply) => {
    void chrome.runtime.lastError;
    managedTab = !!(reply && reply.managed);
    highlightTab = !!(reply && reply.highlight);
    if (reply && reply.activeMs > 0) setActive(reply.activeMs);
  });
})();
