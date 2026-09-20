'use strict';

// Visual feedback layer: a glowing viewport border while acrawl drives this tab,
// click ripples, and boxes over elements that changed after an action. Every
// element is non-interactive and lives in a closed shadow root. The service
// worker (commands/overlay.js) decides when to show things; this file only draws.
(() => {
  const RIPPLE_MS = 700;
  const BOX_MS = 2600;
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
      position: fixed; border: 2px solid var(--c); border-radius: 4px;
      background: color-mix(in srgb, var(--c) 14%, transparent);
      animation: boxfade ${BOX_MS}ms ease-out forwards;
    }
    .box.added { --c: #22c55e; }
    .box.changed { --c: #f59e0b; }
    .box.fill { --c: #3b82f6; animation-duration: ${FLASH_MS}ms; }
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
    if (!box.el.isConnected) {
      box.div.style.display = 'none';
      return;
    }
    const r = topRect(box.el, box.frames);
    const visible = r.width > 0 && r.height > 0
      && r.x < innerWidth && r.y < innerHeight && r.x + r.width > 0 && r.y + r.height > 0;
    box.div.style.display = visible ? '' : 'none';
    Object.assign(box.div.style, boxStyle(r.x, r.y, r.width, r.height));
  }

  function highlight(added, changed) {
    clearBoxes();
    const jobs = [
      ...added.map((ref) => [ref, 'added']),
      ...changed.map((ref) => [ref, 'changed']),
    ].slice(0, MAX_BOXES);
    for (const [ref, kind] of jobs) {
      if (!REF_RE.test(ref)) continue;
      const hit = findRef(ref, document, []);
      // A box over the whole page is noise: the diff root (BODY/HTML) is never a real change.
      if (!hit || hit.el === hit.el.ownerDocument.body || hit.el === hit.el.ownerDocument.documentElement) continue;
      const box = { div: add('box ' + kind, {}), el: hit.el, frames: hit.frames };
      place(box);
      boxes.push(box);
    }
    if (boxes.length === 0) return;
    // Keep boxes glued to their elements while the page scrolls or reflows.
    const until = Date.now() + BOX_MS;
    const tick = () => {
      boxes.forEach(place);
      rafId = Date.now() < until ? requestAnimationFrame(tick) : 0;
    };
    rafId = requestAnimationFrame(tick);
    // rAF is throttled in background tabs, so removal is timer-driven.
    const mine = boxes;
    setTimeout(() => {
      if (boxes === mine) clearBoxes();
    }, BOX_MS + 100);
  }

  chrome.runtime.onMessage.addListener((msg, _sender, respond) => {
    if (!msg || msg.type !== 'acrawl_overlay') return;
    switch (msg.op) {
      case 'active': setActive(msg.ttl); break;
      case 'hide': hideBorder(); clearBoxes(); break;
      case 'ripple': ripple(msg.x, msg.y, msg.kind); break;
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
    if (reply && reply.activeMs > 0) setActive(reply.activeMs);
  });
})();
