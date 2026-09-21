'use strict';

// Service-worker side of the visual feedback layer (see content_overlay.js).
// Everything here is best-effort: a tab without the content script (opened
// before the extension loaded, or a restricted page) is silently skipped.

const OVERLAY_ACTIVE_MS = 8000;
const overlaySettings = { showIndicators: true, highlightChanges: true };
const overlayActiveUntil = new Map(); // tabId -> ms timestamp

chrome.storage.local.get(overlaySettings, (items) => Object.assign(overlaySettings, items));
chrome.storage.onChanged.addListener((changes, area) => {
  if (area !== 'local') return;
  for (const key of Object.keys(overlaySettings)) {
    if (changes[key]) overlaySettings[key] = changes[key].newValue !== false;
  }
});

// One retry: right after a navigation the content script may not be listening yet,
// and a dropped ripple/border is otherwise silent.
function overlaySend(tabId, msg) {
  const send = () => chrome.tabs.sendMessage(tabId, { type: 'acrawl_overlay', ...msg }, { frameId: 0 });
  return send()
    .catch(() => new Promise((resolve) => setTimeout(resolve, 150)).then(send))
    .catch(() => {});
}

// Called on every command: (re)arms the border. The content script expires it
// itself after the TTL, so a dead worker or dropped socket can't leave it stuck.
function overlayActivity(tabId) {
  if (!overlaySettings.showIndicators) return;
  overlayActiveUntil.set(tabId, Date.now() + OVERLAY_ACTIVE_MS);
  overlaySend(tabId, { op: 'active', ttl: OVERLAY_ACTIVE_MS });
}

// Awaited before an interaction runs, so the page's DOM watcher already exists
// when synchronous handlers (input/change, hover menus) mutate the page.
function overlayWatch(tabId) {
  if (!overlaySettings.showIndicators || !overlaySettings.highlightChanges) return Promise.resolve();
  return overlaySend(tabId, { op: 'watch' });
}

function overlayRipple(tabId, x, y, kind = 'click') {
  if (!overlaySettings.showIndicators) return;
  overlaySend(tabId, { op: 'ripple', x, y, kind });
}

function overlayFlash(tabId, rect) {
  if (!overlaySettings.showIndicators || !rect) return;
  overlaySend(tabId, { op: 'flash', rect });
}

function overlayClear(tabId) {
  overlayActiveUntil.delete(tabId);
  return overlaySend(tabId, { op: 'hide' });
}

// Awaited so the overlay is hidden before Page.captureScreenshot runs; the
// model must never see our own border or boxes in its screenshots.
function overlaySuspend(tabId, on) {
  return overlaySend(tabId, { op: 'suspend', on });
}

// Content script asks on load whether its (managed) tab should show the border.
function overlayHello(sender) {
  const tabId = sender?.tab?.id;
  const managed = Object.values(managedTabs).includes(tabId);
  const remaining = (overlayActiveUntil.get(tabId) || 0) - Date.now();
  return {
    activeMs: managed && overlaySettings.showIndicators && remaining > 0 ? remaining : 0,
    managed: managed && overlaySettings.showIndicators,
    highlight: overlaySettings.highlightChanges,
  };
}

async function handleHighlightChanges(tabId, payload) {
  const refs = (list) => (Array.isArray(list) ? list.filter((r) => /^e\d+$/.test(r)) : []);
  const added = refs(payload?.added);
  const changed = refs(payload?.changed);
  if (overlaySettings.highlightChanges && (added.length || changed.length)) {
    await overlaySend(tabId, { op: 'highlight', added, changed });
  }
  return { highlighted: added.length + changed.length };
}
