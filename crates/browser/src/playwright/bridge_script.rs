use std::sync::LazyLock;

use crate::aria_walk_js::ARIA_WALK_JS;

const PLAYWRIGHT_BRIDGE_NODE_SCRIPT_PREFIX: &str = r#"
const readline = require('node:readline');

function parseHeadless() {
  const raw = process.env.HEADLESS;
  if (raw === undefined) return true;
  const v = String(raw).trim().toLowerCase();
  return !(v === 'false' || v === '0' || v === 'no' || v === 'off');
}

async function resolveFillSelector(pg, raw) {
  if (/^[#.\[]/.test(raw) || /[\[\]:>~+]/.test(raw)) return raw;
  const lower = raw.toLowerCase();
  const candidates = [
    `#${raw}`,
    `#${lower}`,
    `[name="${raw}"]`,
    `[name="${lower}"]`,
    `input[name="${raw}"]`,
    `input[name="${lower}"]`,
    `textarea[name="${raw}"]`,
    `textarea[name="${lower}"]`,
    `select[name="${raw}"]`,
    `select[name="${lower}"]`,
    `[placeholder="${raw}"]`,
    `input[aria-label="${raw}"]`,
    `textarea[aria-label="${raw}"]`,
  ];
  for (const sel of candidates) {
    try {
      const el = await pg.$(sel);
      if (el) return sel;
    } catch (_) {}
  }
  try {
    const resolved = await pg.evaluate((labelText) => {
      const labels = document.querySelectorAll('label');
      for (const lbl of labels) {
        const text = lbl.textContent.trim();
        if (text.toLowerCase() === labelText.toLowerCase()) {
          const forAttr = lbl.getAttribute('for');
          if (forAttr) {
            const target = document.getElementById(forAttr);
            if (target) return `#${forAttr}`;
          }
          const input = lbl.querySelector('input, textarea, select');
          if (input) {
            if (input.id) return `#${input.id}`;
            if (input.name) return `[name="${input.name}"]`;
            const type = input.getAttribute('type') || 'text';
            return `label:has-text("${text}") ${input.tagName.toLowerCase()}[type="${type}"]`;
          }
        }
      }
      return null;
    }, raw);
    if (resolved) {
      try {
        const el = await pg.$(resolved);
        if (el) return resolved;
      } catch (_) {}
    }
  } catch (_) {}
  return raw;
}

async function resolveEditorSurface(pg, sel) {
  try {
    const redirected = await pg.evaluate((s) => {
      let el;
      try { el = document.querySelector(s); } catch (_) { return null; }
      if (!el) return null;
      const tag = el.tagName.toLowerCase();
      if (tag !== 'textarea' && tag !== 'input') return null;
      const rect = el.getBoundingClientRect();
      if (rect.width >= 4 && rect.height >= 4) return null; // visible; no redirect needed
      function selectorOf(node) {
        if (node.id) return '#' + CSS.escape(node.id);
        const path = [];
        let cur = node;
        while (cur && cur.parentElement) {
          if (cur.id) { path.unshift('#' + CSS.escape(cur.id)); break; }
          const parent = cur.parentElement;
          const t = cur.tagName.toLowerCase();
          const same = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
          path.unshift(same.length > 1 ? `${t}:nth-of-type(${same.indexOf(cur) + 1})` : t);
          cur = parent;
        }
        return path.join(' > ');
      }
      // Phase 1: walk ancestors for any contenteditable
      let cur = el.parentElement;
      while (cur && cur !== document.body) {
        if (cur.getAttribute('contenteditable') === 'true') return selectorOf(cur);
        cur = cur.parentElement;
      }
      // Phase 2: proximity walk — expand outward from el's parent until we find
      // the smallest container holding exactly one visible interactive element.
      // Framework-agnostic: handles CM5 (sibling <textarea>), CM6/ProseMirror/
      // Quill (contenteditable cousin), and any editor we haven't seen yet.
      let container = el.parentElement;
      while (container && container !== document.body) {
        const candidates = Array.from(container.querySelectorAll(
          '[contenteditable="true"], textarea, input:not([type="hidden"])'
        )).filter(c => {
          if (c === el) return false;
          const r = c.getBoundingClientRect();
          return r.width >= 4 && r.height >= 4;
        });
        if (candidates.length === 1) return selectorOf(candidates[0]);
        if (candidates.length > 1) {
          // Prefer contenteditable — more likely to be the editor surface than
          // a sibling input/textarea that belongs to a different field.
          const ce = candidates.filter(c => c.getAttribute('contenteditable') === 'true');
          if (ce.length === 1) return selectorOf(ce[0]);
          // Ambiguous: multiple candidates with no clear winner; don't guess.
          return null;
        }
        container = container.parentElement;
      }
      return null;
    }, sel);
    if (redirected) return redirected;
  } catch (_) {}
  return sel;
}

async function tryRichEditorFill(pg, sel, value) {
  // Step 1: CodeMirror v5 JS API — instant, no typing latency
  try {
    const cm5ok = await pg.evaluate(({ s, v }) => {
      let el;
      try { el = document.querySelector(s); } catch (_) {}
      const cm = el?.closest?.('.CodeMirror');
      if (cm?.CodeMirror) {
        cm.CodeMirror.setValue(v);
        cm.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      }
      return false;
    }, { s: sel, v: value });
    if (cm5ok) return 'codemirror5';
  } catch (_) {}

  // Step 2: any contenteditable via execCommand — covers CM6, ProseMirror, TipTap, Quill, Lexical, Slate
  try {
    const ceOk = await pg.evaluate(({ s, v }) => {
      let el;
      try { el = document.querySelector(s); } catch (_) {}
      let ce = el;
      while (ce && ce !== document.body) {
        if (ce.getAttribute('contenteditable') === 'true') break;
        ce = ce.parentElement;
      }
      if (!ce || ce === document.body) {
        // Do not fall back to document.body — that would write to an unrelated editor.
        if (!el) return false;
        const root = el.closest('[class*="editor"], [role="textbox"]');
        ce = root ? root.querySelector('[contenteditable="true"]') : null;
      }
      if (!ce || ce === document.body) return false;
      ce.focus();
      document.execCommand('selectAll', false, null);
      return document.execCommand('insertText', false, v);
    }, { s: sel, v: value });
    if (ceOk) return 'contenteditable';
  } catch (_) {}

  // Step 3: focus + Ctrl+A + keyboard.type — universal last resort
  try {
    const focused = await pg.evaluate(({ s }) => {
      let el;
      try { el = document.querySelector(s); } catch (_) {}
      let ce = el;
      while (ce && ce !== document.body) {
        if (ce.getAttribute('contenteditable') === 'true') { ce.focus(); return true; }
        ce = ce.parentElement;
      }
      // Do not fall back to document.body — that would write to an unrelated editor.
      if (!el) return false;
      const root = el.closest('[class*="editor"], [role="textbox"]');
      const found = root ? root.querySelector('[contenteditable="true"]') : null;
      if (found) { found.focus(); return true; }
      return false;
    }, { s: sel });
    if (focused) {
      await pg.keyboard.press('ControlOrMeta+a'); // portable: Ctrl on Linux/Win, Cmd on macOS
      await pg.keyboard.type(value, { delay: 0 });
      return 'keyboard';
    }
  } catch (_) {}

  return null;
}

const observationBuffers = new Map();
const MAX_OBSERVATION_BYTES = 2 * 1024 * 1024;
let currentSeq = 0;
let nextRequestId = 0;
let nextWebSocketId = 0;
let interceptRulesMap = {};

function estimateEventBytes(event) {
  try {
    return Buffer.byteLength(JSON.stringify(event), 'utf8');
  } catch (_) {
    return 0;
  }
}

function bufferEvent(pageIndex, event, seqAtInitiation = currentSeq) {
  event.seq_at_initiation = seqAtInitiation;

  if (!observationBuffers.has(pageIndex)) {
    observationBuffers.set(pageIndex, { events: [], currentBytes: 0 });
  }
  const buf = observationBuffers.get(pageIndex);
  const eventBytes = estimateEventBytes(event);

  buf.events.push(event);
  buf.currentBytes += eventBytes;

  while (buf.currentBytes > MAX_OBSERVATION_BYTES && buf.events.length > 0) {
    const removed = buf.events.shift();
    buf.currentBytes = Math.max(0, buf.currentBytes - estimateEventBytes(removed));
  }
}

function truncateText(value, maxChars) {
  if (maxChars === undefined) maxChars = 8192;
  const text = (typeof value === 'string') ? value : String(value === null || value === undefined ? '' : value);
  return text.length > maxChars ? text.slice(0, maxChars) + '...[truncated]' : text;
}

function toNullableInt(value) {
  return Number.isInteger(value) ? value : null;
}

const REQUEST_BODY_CAP = 16384;
const RESPONSE_BODY_CAP = 16384;

// Playwright ResourceTiming fields are millisecond offsets from startTime, or
// -1 when a phase did not occur (cache hit, reused connection, no TLS). span()
// returns a duration only when both ends are present and ordered.
function computeTimingMs(t) {
  if (!t) return null;
  const span = (start, end) =>
    (typeof start === 'number' && typeof end === 'number' && start >= 0 && end >= start)
      ? Math.round(end - start)
      : null;
  const hasTls = typeof t.secureConnectionStart === 'number' && t.secureConnectionStart >= 0;
  return {
    dns_ms: span(t.domainLookupStart, t.domainLookupEnd),
    connect_ms: span(t.connectStart, t.connectEnd),
    tls_ms: hasTls ? span(t.secureConnectionStart, t.connectEnd) : null,
    ttfb_ms: span(t.requestStart, t.responseStart),
    download_ms: span(t.responseStart, t.responseEnd),
  };
}

function isTextualContentType(contentType) {
  if (!contentType) return false;
  const c = String(contentType).toLowerCase();
  return c.includes('text/') || c.includes('json') || c.includes('xml')
    || c.includes('javascript') || c.includes('ecmascript') || c.includes('html')
    || c.includes('css') || c.includes('graphql') || c.includes('urlencoded')
    || c.includes('csv');
}

// CloakBrowser (rebrowser-style stealth) suppresses CDP Runtime event *delivery*
// (Runtime.consoleAPICalled / exceptionThrown / bindingCalled) on every CDP
// session -- including a freshly created one -- while still ACKing Runtime.enable.
// So page.on('console')/'pageerror', a dedicated newCDPSession, and exposeBinding
// all capture nothing from page JS. Runtime.evaluate is NOT suppressed, so we
// install a page-context init-script that mirrors console.* and the global
// error/unhandledrejection events into a page-global ring buffer, then pull it via
// page.evaluate (drainConsolePage) on poll and before each navigation. Tradeoff:
// console.* is no longer a native function (a minor fingerprint vector shared by
// Sentry/LogRocket-style monitoring) -- accepted, since the CDP path yields zero
// events under CloakBrowser.
const CONSOLE_CAPTURE_SOURCE = `
  (() => {
    if (window.__acrawlConsoleHooked) return;
    window.__acrawlConsoleHooked = true;
    var BUF = [];
    var MAX = 2000;
    window.__acrawlConsoleBuffer = BUF;
    function fmt(a) {
      if (typeof a === 'string') return a;
      if (a instanceof Error) return (a.stack || a.message || String(a));
      try { return JSON.stringify(a); } catch (_) { return String(a); }
    }
    function push(entry) {
      BUF.push(entry);
      if (BUF.length > MAX) BUF.splice(0, BUF.length - MAX);
    }
    ['log', 'info', 'warn', 'error', 'debug', 'trace'].forEach(function (m) {
      var orig = console[m];
      console[m] = function () {
        try {
          push({ level: m, message_type: 'Console', text: Array.prototype.slice.call(arguments).map(fmt).join(' '), ts: Date.now() });
        } catch (_) {}
        if (typeof orig === 'function') return orig.apply(console, arguments);
      };
    });
    window.addEventListener('error', function (e) {
      try {
        push({
          level: 'error',
          message_type: 'Exception',
          text: (e && e.message) ? String(e.message) : 'Uncaught error',
          source_url: (e && e.filename) ? e.filename : null,
          source_line: (e && typeof e.lineno === 'number') ? e.lineno : null,
          source_column: (e && typeof e.colno === 'number') ? e.colno : null,
          stack: (e && e.error && e.error.stack) ? String(e.error.stack) : null,
          ts: Date.now(),
        });
      } catch (_) {}
    });
    window.addEventListener('unhandledrejection', function (e) {
      try {
        var reason = e ? e.reason : null;
        var text = (reason instanceof Error) ? (reason.message || String(reason)) : fmt(reason);
        push({ level: 'error', message_type: 'PromiseRejection', text: 'Unhandled promise rejection: ' + text, stack: (reason instanceof Error && reason.stack) ? String(reason.stack) : null, ts: Date.now() });
      } catch (_) {}
    });
  })();
`;

const DRAIN_CONSOLE_JS = "(() => { var b = window.__acrawlConsoleBuffer || []; window.__acrawlConsoleBuffer = []; return b; })()";

// Pull page-buffered console/error/rejection entries (see CONSOLE_CAPTURE_SOURCE)
// into the Node-side observation buffer. The page global resets on navigation, so
// this is invoked before navigate/reload/go_back and on every poll.
async function drainConsolePage(page, pageIndex) {
  if (!page) return;
  let entries;
  try {
    entries = await page.evaluate(DRAIN_CONSOLE_JS);
  } catch (_) {
    return;
  }
  if (!Array.isArray(entries)) return;
  for (const entry of entries) {
    if (!entry) continue;
    bufferEvent(pageIndex, {
      type: 'ConsoleMessage',
      timestamp_ms: typeof entry.ts === 'number' ? entry.ts : Date.now(),
      tab_index: pageIndex,
      level: entry.level ? String(entry.level) : 'info',
      message_type: entry.message_type || 'Console',
      text: truncateText(entry.text !== undefined && entry.text !== null ? entry.text : ''),
      source_url: entry.source_url ? entry.source_url : null,
      source_line: toNullableInt(entry.source_line),
      source_column: toNullableInt(entry.source_column),
      stack: entry.stack ? truncateText(entry.stack, 16384) : null,
    });
  }
}

// `pages` can shift a page's index (close_page splices the array), so the
// listeners below re-resolve the page's current index via currentPageIndex()
// close to where each event is actually buffered, instead of capturing
// `initialPageIndex` once -- otherwise they'd keep buffering events under a
// stale index forever, out of step with observationBuffers (which close_page
// re-keys to the post-splice scheme). Handlers with an await chain between
// entry and their bufferEvent call (e.g. requestfinished) re-resolve right
// before buffering, since a close_page on another tab can shift this page's
// index while those awaits are in flight.
async function attachObservationListeners(page, pages, initialPageIndex) {
  if (page.__acrawlObservationAttached) {
    return;
  }
  page.__acrawlObservationAttached = true;

  const pendingRequests = new WeakMap();
  // pages.indexOf(page) stops resolving once this page itself is closed and
  // spliced out, so we fall back to the last index we actually observed the
  // page occupy -- not the immutable initialPageIndex, which may be stale by
  // then if an earlier close_page already shifted this page at least once.
  let lastKnownPageIndex = initialPageIndex;
  const currentPageIndex = () => {
    const idx = pages.indexOf(page);
    if (idx !== -1) lastKnownPageIndex = idx;
    return idx === -1 ? lastKnownPageIndex : idx;
  };

  page.on('request', (req) => {
    if (req.url().includes('__acrawl_poll')) return;
    const pageIndex = currentPageIndex();

    const requestId = `req_${pageIndex}_${++nextRequestId}`;
    const startTime = Date.now();
    const seqAtInitiation = currentSeq;
    pendingRequests.set(req, { requestId, startTime, seqAtInitiation });

    const serviceWorker = typeof req.serviceWorker === 'function' ? req.serviceWorker() : null;
    const initiator = typeof req.initiator === 'function' ? req.initiator() : null;

    bufferEvent(pageIndex, {
      type: 'NetworkRequest',
      timestamp_ms: startTime,
      tab_index: pageIndex,
      request_id: requestId,
      url: req.url(),
      method: req.method(),
      status: null,
      state: 'Pending',
      size_bytes: null,
      duration_ms: null,
      request_type: req.resourceType(),
      from_service_worker: Boolean(serviceWorker),
      initiator_type: initiator?.type ?? null,
      reason: null,
    }, seqAtInitiation);
  });

  page.on('requestfinished', async (req) => {
    if (req.url().includes('__acrawl_poll')) return;

    const tracked = pendingRequests.get(req);
    const response = await req.response().catch(() => null);
    let sizeBytes = null;
    if (response) {
      try {
        const contentLength = await response.headerValue('content-length');
        if (contentLength !== null) {
          const parsed = Number.parseInt(contentLength, 10);
          sizeBytes = Number.isNaN(parsed) ? null : parsed;
        }
      } catch (_) {}
    }

    // inspect_request runs long after the Response object is gone, so headers,
    // timing and bodies are pulled here at completion and buffered. Bodies are
    // restricted to textual content types and truncated to bound buffer growth.
    let requestHeaders = null;
    try { requestHeaders = await req.allHeaders(); } catch (_) {}
    let responseHeaders = null;
    if (response) {
      try { responseHeaders = await response.allHeaders(); } catch (_) {}
    }

    let requestBody = null;
    try {
      const postData = req.postData();
      if (postData) requestBody = truncateText(postData, REQUEST_BODY_CAP);
    } catch (_) {}

    let responseBody = null;
    if (response) {
      const contentType = responseHeaders ? (responseHeaders['content-type'] || '') : '';
      if (isTextualContentType(contentType)) {
        try {
          const body = await response.body();
          if (body) responseBody = truncateText(body.toString('utf8'), RESPONSE_BODY_CAP);
        } catch (_) {}
      }
    }

    let timing = null;
    try { timing = computeTimingMs(req.timing()); } catch (_) {}

    // Re-resolve now, right before buffering -- the awaits above give
    // close_page on another tab a chance to shift this page's index while
    // this handler was in flight, so the value captured at handler entry
    // could already be stale by the time we get here.
    const pageIndex = currentPageIndex();

    bufferEvent(pageIndex, {
      type: 'NetworkRequest',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      request_id: tracked?.requestId ?? `req_${pageIndex}_${++nextRequestId}`,
      url: req.url(),
      method: req.method(),
      status: response?.status() ?? null,
      state: 'Completed',
      size_bytes: sizeBytes,
      duration_ms: tracked ? Math.max(0, Date.now() - tracked.startTime) : null,
      request_type: req.resourceType(),
      from_service_worker: false,
      initiator_type: null,
      reason: null,
      timing,
      request_headers: requestHeaders,
      response_headers: responseHeaders,
      request_body: requestBody,
      response_body: responseBody,
    }, tracked?.seqAtInitiation ?? currentSeq);

    pendingRequests.delete(req);
  });

  page.on('requestfailed', (req) => {
    if (req.url().includes('__acrawl_poll')) return;
    const pageIndex = currentPageIndex();

    const tracked = pendingRequests.get(req);

    bufferEvent(pageIndex, {
      type: 'NetworkRequest',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      request_id: tracked?.requestId ?? `req_${pageIndex}_${++nextRequestId}`,
      url: req.url(),
      method: req.method(),
      status: null,
      state: 'Failed',
      size_bytes: null,
      duration_ms: tracked ? Math.max(0, Date.now() - tracked.startTime) : null,
      request_type: req.resourceType(),
      from_service_worker: false,
      initiator_type: null,
      reason: req.failure()?.errorText || 'Unknown',
    }, tracked?.seqAtInitiation ?? currentSeq);

    pendingRequests.delete(req);
  });

  page.on('websocket', (ws) => {
    const pageIndex = currentPageIndex();
    const wsId = `ws_${pageIndex}_${++nextWebSocketId}`;

    ws.on('framesent', (frame) => {
      const pageIndex = currentPageIndex();
      const payload = frame.payload?.toString() || '';
      bufferEvent(pageIndex, {
        type: 'WebSocketFrame',
        timestamp_ms: Date.now(),
        tab_index: pageIndex,
        connection_id: wsId,
        url: ws.url(),
        direction: 'sent',
        data: payload,
        size_bytes: payload.length,
        connection_status: 'open',
      });
    });

    ws.on('framereceived', (frame) => {
      const pageIndex = currentPageIndex();
      const payload = frame.payload?.toString() || '';
      bufferEvent(pageIndex, {
        type: 'WebSocketFrame',
        timestamp_ms: Date.now(),
        tab_index: pageIndex,
        connection_id: wsId,
        url: ws.url(),
        direction: 'received',
        data: payload,
        size_bytes: payload.length,
        connection_status: 'open',
      });
    });
  });
}

async function bootstrap() {
  let launch;
  try {
    ({ launch } = await import('cloakbrowser'));
  } catch (_firstError) {
    // ESM import() does not respect NODE_PATH — manually resolve from it.
    const path = require('node:path');
    const fs = require('node:fs');
    const url = require('node:url');
    let resolved = false;
    for (const dir of (process.env.NODE_PATH || '').split(path.delimiter)) {
      if (!dir) continue;
      const pkgJson = path.join(dir, 'cloakbrowser', 'package.json');
      if (!fs.existsSync(pkgJson)) continue;
      try {
        const pkg = JSON.parse(fs.readFileSync(pkgJson, 'utf8'));
        const entry = pkg.exports?.['.']?.import || pkg.module || pkg.main || 'index.js';
        ({ launch } = await import(url.pathToFileURL(path.join(dir, 'cloakbrowser', entry)).href));
        resolved = true;
        break;
      } catch (_) {}
    }
    if (!resolved) {
      process.stdout.write(JSON.stringify({
        event: 'bridge_bootstrap',
        ok: false,
        error: {
          kind: 'playwright_not_installed',
          message: 'CloakBrowser package not found. Install with `npm install cloakbrowser`.'
        }
      }) + '\n');
      process.exit(1);
      return;
    }
  }
  console.log = (...args) => process.stderr.write(args.map(String).join(' ') + '\n');
  const browser = await launch({ headless: parseHeadless(), humanize: true });
  let context = await browser.newContext({ viewport: { width: 1920, height: 955 }, screen: { width: 1920, height: 1080 } });
  await context.addInitScript(`
    (() => {
      // Spoof screen dimensions by shadowing them on the REAL screen object.
      // Replacing window.screen with Object.create(Screen.prototype, ...) loses
      // the internal Screen slot, so inherited native accessors like
      // screen.orientation throw "Illegal invocation" (which breaks axe-core).
      const dims = { width: 1920, height: 1080, availWidth: 1920, availHeight: 1040, colorDepth: 24, pixelDepth: 24 };
      for (const k of Object.keys(dims)) {
        try { Object.defineProperty(window.screen, k, { value: dims[k], enumerable: true, configurable: true }); } catch (_) {}
      }
    })();
  `);
  await context.addInitScript(CONSOLE_CAPTURE_SOURCE);
  let page = await context.newPage();
  const pages = [page];
  await attachObservationListeners(page, pages, 0);
  context.on('page', (p) => {
    if (!pages.includes(p)) {
      pages.push(p);
      const popupIndex = pages.length - 1;
      void attachObservationListeners(p, pages, popupIndex).catch((e) => { process.stderr.write('[acrawl] popup observation attach failed: ' + String(e) + '\n'); });
    }
  });

  function activePageIndex() {
    const idx = pages.indexOf(page);
    return idx === -1 ? 0 : idx;
  }
  process.stdout.write(JSON.stringify({ event: 'bridge_bootstrap', ok: true }) + '\n');

  async function bypassTurnstileIfPresent(pg) {
    let html = await pg.content();
    if (!html.includes('Checking your browser') && !html.includes('challenge-platform')) {
      return html;
    }
    await pg.mouse.move(120 + Math.random() * 200, 180 + Math.random() * 150);
    await new Promise(r => setTimeout(r, 500 + Math.random() * 800));
    await pg.mouse.move(350 + Math.random() * 250, 280 + Math.random() * 180);
    await new Promise(r => setTimeout(r, 400 + Math.random() * 600));
    for (let i = 0; i < 16; i++) {
      await new Promise(r => setTimeout(r, 500));
      html = await pg.content();
      if (!html.includes('Checking your browser')) break;
    }
    return html;
  }

  const wire = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of wire) {
    let command;
    try {
      command = JSON.parse(line);
    } catch (error) {
      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: false,
        error: { kind: 'invalid_json', message: String(error) }
      }) + '\n');
      continue;
    }

    if (command.action === 'navigate') {
      try {
        await drainConsolePage(page, activePageIndex());
        await page.goto(command.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        // Wait for SPA API calls to complete. Cap at 5s so pages with
        // persistent connections (WebSocket, SSE, polling) don't hang.
        try {
          await page.waitForLoadState('networkidle', { timeout: 5000 });
        } catch (_) { /* networkidle timed out — proceed with current state */ }
        // For SPAs that render content asynchronously after XHR (e.g. Gitee
        // search), poll for visible text content to appear before capturing.
        // Threshold matches MIN_VISIBLE_CHARS_THRESHOLD in fetch.rs.
        try {
          const MIN_VISIBLE_CHARS = 200;
          const pollDeadline = Date.now() + 3000;
          while (Date.now() < pollDeadline) {
            const textLen = await page.evaluate(() => (document.body?.innerText?.trim()?.length ?? 0));
            if (textLen >= MIN_VISIBLE_CHARS) break;
            await new Promise(r => setTimeout(r, 300));
          }
        } catch (_) {}
        // `bypassTurnstileIfPresent` already calls `page.content()` after
        // any nudge it performs, so reuse that html instead of fetching
        // again — the earlier `html = await page.content()` overwrite was
        // dead and just doubled the wire-format round-trip.
        const html = await bypassTurnstileIfPresent(page);
        const title = await page.title();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { title, html }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'navigate_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'reload') {
      try {
        await drainConsolePage(page, activePageIndex());
        await page.reload({ waitUntil: 'domcontentloaded', timeout: 30000 });
        // Wait for SPA API calls to complete. Cap at 5s so pages with
        // persistent connections (WebSocket, SSE, polling) don't hang.
        try {
          await page.waitForLoadState('networkidle', { timeout: 5000 });
        } catch (_) { /* networkidle timed out — proceed with current state */ }
        const html = await bypassTurnstileIfPresent(page);
        const title = await page.title();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { title, html }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'reload_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'close') {
      await page.close().catch(() => {});
      await browser.close().catch(() => {});
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { closed: true } }) + '\n');
      process.exit(0);
    }

    if (command.action === 'new_page') {
      try {
        const newPage = await context.newPage();
        if (!pages.includes(newPage)) {
          pages.push(newPage);
        }
        const pageIndex = pages.indexOf(newPage);
        await attachObservationListeners(newPage, pages, pageIndex);
        page = newPage;
        let currentUrl = newPage.url();
        if (command.url) {
          await newPage.goto(command.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
          currentUrl = newPage.url();
          await bypassTurnstileIfPresent(newPage);
        }
        await newPage.bringToFront();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { pageIndex, url: currentUrl }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'new_page_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'close_page') {
      try {
        const pageIndex = command.pageIndex;
        if (!Number.isInteger(pageIndex) || pageIndex < 0 || pageIndex >= pages.length || !pages[pageIndex]) {
          process.stdout.write(JSON.stringify({
            event: 'bridge_response',
            ok: false,
            error: { kind: 'close_page_failed', message: `Invalid page index ${pageIndex}` }
          }) + '\n');
          continue;
        }
        const targetPage = pages[pageIndex];
        await targetPage.close();
        pages.splice(pageIndex, 1);
        // Shift observation buffer indices for pages after the removed one
        const shiftedBufs = new Map();
        for (const [idx, buf] of observationBuffers.entries()) {
          if (idx > pageIndex) shiftedBufs.set(idx - 1, buf);
          else if (idx < pageIndex) shiftedBufs.set(idx, buf);
        }
        observationBuffers.clear();
        for (const [k, v] of shiftedBufs) observationBuffers.set(k, v);
        if (page === targetPage) {
          if (pages.length > 0) {
            page = pages[Math.min(pageIndex, pages.length - 1)];
            await page.bringToFront().catch(() => {});
          }
        }
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { closed: true }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'close_page_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'click') {
      try {
        const urlBefore = page.url();
        await page.click(command.selector, { timeout: 5000 });
        const deadline = Date.now() + 2000;
        while (Date.now() < deadline) {
          if (page.url() !== urlBefore) {
            await page.waitForLoadState('domcontentloaded').catch(() => {});
            break;
          }
          await new Promise(r => setTimeout(r, 50));
        }
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true } }) + '\n');
      } catch (mainError) {
        let clickedInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.click(command.selector, { timeout: 2000 });
            clickedInFrame = true;
            break;
          } catch (_) {}
        }
        if (clickedInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'click_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'click_at') {
      try {
        await page.mouse.click(command.x, command.y);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true, x: command.x, y: command.y } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'click_at_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'fill') {
      let sel = command.selector;
      try {
        sel = await resolveFillSelector(page, sel);
        sel = await resolveEditorSurface(page, sel);
        await page.fill(sel, command.value, { timeout: 5000 });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, resolvedSelector: sel } }) + '\n');
      } catch (mainError) {
        try {
          const richEditorResult = await tryRichEditorFill(page, sel, command.value);
          if (richEditorResult) {
            process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, richEditor: richEditorResult } }) + '\n');
            continue;
          }
        } catch (_) {}
        let filledInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.fill(sel, command.value, { timeout: 2000 });
            filledInFrame = true;
            break;
          } catch (_) {}
        }
        if (filledInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'fill_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'screenshot') {
      try {
        const opts = { type: command.format || 'png' };
        if (command.quality && (opts.type === 'jpeg' || opts.type === 'webp')) opts.quality = command.quality;
        if (command.fullPage) opts.fullPage = true;
        let buffer;
        if (command.selector) {
          buffer = await page.locator(command.selector).screenshot(opts);
        } else {
          buffer = await page.screenshot(opts);
        }
        const base64Data = buffer.toString('base64');
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { screenshot_base64: base64Data, size_bytes: buffer.length } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'screenshot_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'go_back') {
      try {
        await drainConsolePage(page, activePageIndex());
        await page.goBack({ waitUntil: 'domcontentloaded', timeout: 30000 });
        const url = page.url();
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'go_back_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'scroll') {
      try {
        const dir = command.direction === 'up' ? -1 : 1;
        const px = (command.pixels || 500) * dir;
        await page.evaluate((y) => window.scrollBy(0, y), px);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { scrolled: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'scroll_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'page_map') {
      try {
        const scope = command.scope || null;
        const depth = Number.isFinite(command.depth) ? Math.min(Math.max(Math.floor(command.depth), 1), 10) : 5;
        const result = await page.evaluate(({scope, depth}) => {
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

"#;

#[allow(clippy::needless_raw_string_hashes)]
pub(crate) static PLAYWRIGHT_BRIDGE_NODE_SCRIPT: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{PLAYWRIGHT_BRIDGE_NODE_SCRIPT_PREFIX}{ARIA_WALK_JS}{PLAYWRIGHT_BRIDGE_NODE_SCRIPT_SUFFIX}"
    )
});

const PLAYWRIGHT_BRIDGE_NODE_SCRIPT_SUFFIX: &str = r#"

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
        }, {scope, depth});
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'page_map_error', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'extract_dom_snapshot') {
      try {
        const scope = command.scope || null;
        const result = await page.evaluate((scope) => {
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
            if (el.hidden) return false;
            const style = getComputedStyle(el);
            if (style.display === 'none' || style.visibility === 'hidden') return false;
            const rect = el.getBoundingClientRect();
            return rect.width > 0 || rect.height > 0;
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
        }, scope);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'extract_dom_snapshot_error', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'read_content') {
      try {
        const heading = command.heading || null;
        const selector = command.selector || null;
        const offset = command.offset || 0;
        const max_chars = command.max_chars || 10000;
        const result = await page.evaluate(({ heading, selector, offset, max_chars }) => {
          let rawContent = '';
          let matches_count = 0;
          let found = false;
          if (heading) {
            const allHeadings = Array.from(document.querySelectorAll('h1,h2,h3,h4,h5,h6'));
            const matches = allHeadings.filter(el => el.innerText.trim().toLowerCase() === heading.toLowerCase());
            matches_count = matches.length;
            if (matches.length > 0) {
              found = true;
              const el = matches[0];
              const level = parseInt(el.tagName[1]);
              let sibling = el.nextElementSibling;
              while (sibling) {
                const sibTag = sibling.tagName.toLowerCase();
                if (/^h[1-6]$/.test(sibTag) && parseInt(sibTag[1]) <= level) break;
                rawContent += (sibling.innerText || '') + '\n';
                sibling = sibling.nextElementSibling;
              }
            } else {
              const hint = allHeadings.slice(0, 20).map(el => el.innerText.trim());
              return { content: '', found: false, total_chars: 0, offset: 0, has_more: false, truncated: false, matches_count: 0, hint };
            }
          } else if (selector) {
            const els = Array.from(document.querySelectorAll(selector));
            matches_count = els.length;
            found = els.length > 0;
            rawContent = els.map(el => el.innerText || '').join('\n');
          }
          const total_chars = rawContent.length;
          const sliced = rawContent.slice(offset, offset + max_chars);
          const has_more = offset + max_chars < total_chars;
          const truncated = sliced.length < (total_chars - offset);
          return { content: sliced, found, total_chars, offset, has_more, truncated, matches_count };
        }, { heading, selector, offset, max_chars });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'read_content_error', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'wait_for_selector') {
      try {
        const timeout = command.timeout_ms || 5000;
        const opts = { timeout };
        if (command.state) opts.state = command.state;
        await page.waitForSelector(command.selector, opts);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: false } }) + '\n');
      }
      continue;
    }

    if (command.action === 'select_option') {
      try {
        await page.selectOption(command.selector, command.value);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (mainError) {
        let selectedInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.selectOption(command.selector, command.value);
            selectedInFrame = true;
            break;
          } catch (_) {}
        }
        if (selectedInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'select_option_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'evaluate') {
      try {
        const result = await page.evaluate(command.script);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { value: result } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'evaluate_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'hover') {
      try {
        await page.hover(command.selector);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (mainError) {
        let hoveredInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.hover(command.selector);
            hoveredInFrame = true;
            break;
          } catch (_) {}
        }
        if (hoveredInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'hover_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'press_key') {
      try {
        if (command.selector) {
          await page.focus(command.selector);
        }
        await page.keyboard.press(command.key);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (mainError) {
        let pressedInFrame = false;
        if (command.selector) {
          for (const frame of page.frames()) {
            if (frame === page.mainFrame()) continue;
            try {
              await frame.focus(command.selector);
              await page.keyboard.press(command.key);
              pressedInFrame = true;
              break;
            } catch (_) {}
          }
        }
        if (pressedInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'press_key_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'list_resources') {
      try {
        const resources = await page.evaluate(() => {
          const links = Array.from(document.querySelectorAll('a[href]')).map(a => ({ href: a.href, text: a.textContent.trim() }));
          const images = Array.from(document.querySelectorAll('img')).map(img => ({ src: img.src, alt: img.alt }));
          const forms = Array.from(document.querySelectorAll('form')).map(f => ({ action: f.action, method: f.method, id: f.id }));
          return { links, images, forms };
        });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: resources }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'list_resources_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'save_file') {
      try {
        const fs = require('node:fs');
        const nodePath = require('node:path');
        const response = await context.request.get(command.url, { headers: command.headers || {}, timeout: 30000 });
        if (!response.ok()) {
          throw new Error(`HTTP ${response.status()} ${response.statusText()} for ${command.url}`);
        }
        const body = await response.body();
        const dir = nodePath.dirname(command.path);
        if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(command.path, body);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { path: command.path, size_bytes: body.length } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'save_file_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'switch_tab') {
      try {
        const idx = command.index === undefined ? -1 : command.index;
        const targetIdx = idx === -1 ? pages.length - 1 : idx;
        if (targetIdx < 0 || targetIdx >= pages.length || !pages[targetIdx]) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: `Invalid tab index ${idx}, have ${pages.length} tab(s)` } }) + '\n');
        } else {
          page = pages[targetIdx];
          await page.bringToFront();
          const url = page.url();
          const title = await page.title();
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url, title, tab_count: pages.length, pageIndex: targetIdx } }) + '\n');
        }
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'export_cookies') {
      try {
        const cookies = await context.cookies();
        let localStorage = {};
        try {
          localStorage = await page.evaluate(() => {
            const result = {};
            for (let i = 0; i < window.localStorage.length; i++) {
              const key = window.localStorage.key(i);
              if (key !== null) result[key] = window.localStorage.getItem(key) || '';
            }
            return result;
          });
        } catch (_) { /* localStorage may be unavailable on some pages */ }
        const url = page.url();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { cookies, local_storage: localStorage, url }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'export_cookies_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'import_cookies') {
      try {
        if (command.cookies && command.cookies.length > 0) {
          await context.addCookies(command.cookies);
        }
        if (command.local_storage && typeof command.local_storage === 'object') {
          await page.evaluate((ls) => {
            for (const [k, v] of Object.entries(ls)) {
              try { window.localStorage.setItem(k, v); } catch (_) {}
            }
          }, command.local_storage);
        }
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { imported: true }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'import_cookies_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'set_device') {
      try {
        const cookies = await context.cookies();
        let localStorage = {};
        try {
          localStorage = await page.evaluate(() => {
            const result = {};
            for (let i = 0; i < window.localStorage.length; i++) {
              const key = window.localStorage.key(i);
              if (key !== null) result[key] = window.localStorage.getItem(key) || '';
            }
            return result;
          });
        } catch (_) { /* localStorage may be unavailable */ }
        const currentUrl = page.url();

        const ctxOpts = {};
        if (command.viewport) ctxOpts.viewport = command.viewport;
        if (command.screen) ctxOpts.screen = command.screen;
        if (command.userAgent) ctxOpts.userAgent = command.userAgent;
        if (command.deviceScaleFactor !== undefined) ctxOpts.deviceScaleFactor = command.deviceScaleFactor;
        if (command.isMobile !== undefined) ctxOpts.isMobile = command.isMobile;
        if (command.hasTouch !== undefined) ctxOpts.hasTouch = command.hasTouch;

        let storageOrigin = null;
        if (currentUrl && currentUrl !== 'about:blank') {
          try { storageOrigin = new URL(currentUrl).origin; } catch (_) {}
        }
        const lsEntries = Object.entries(localStorage);
        if (cookies.length > 0 || lsEntries.length > 0) {
          const origins = (lsEntries.length > 0 && storageOrigin && storageOrigin !== 'null')
            ? [{ origin: storageOrigin, localStorage: lsEntries.map(([name, value]) => ({ name, value })) }]
            : [];
          ctxOpts.storageState = { cookies, origins };
        }

        // Build new context BEFORE closing old — rollback-safe
        const newContext = await browser.newContext(ctxOpts);
        const newPage = await newContext.newPage();
        await newContext.addInitScript(CONSOLE_CAPTURE_SOURCE);

        if (command.screen) {
          await newContext.addInitScript(`
            (() => {
              // Shadow dims on the real screen object (preserves screen.orientation
              // and other native accessors; see bootstrap note).
              const dims = { width: ${command.screen.width}, height: ${command.screen.height}, availWidth: ${command.screen.width}, availHeight: ${command.screen.height}, colorDepth: 24, pixelDepth: 24 };
              for (const k of Object.keys(dims)) {
                try { Object.defineProperty(window.screen, k, { value: dims[k], enumerable: true, configurable: true }); } catch (_) {}
              }
            })();
          `);
        }

        // Restore localStorage manually (storageState only seeds on first navigation)
        if (lsEntries.length > 0 && storageOrigin && storageOrigin !== 'null') {
          try {
            await newPage.goto(storageOrigin, { waitUntil: 'commit', timeout: 10000 });
            await newPage.evaluate((entries) => {
              for (const [k, v] of entries) window.localStorage.setItem(k, v);
            }, lsEntries);
          } catch (_) { /* best-effort localStorage restore */ }
        }

        if (currentUrl && currentUrl !== 'about:blank') {
          try {
            await newPage.goto(currentUrl, { waitUntil: 'domcontentloaded', timeout: 30000 });
          } catch (navErr) {
            // Navigation failed — tear down new context and keep old one intact
            await newContext.close().catch(() => {});
            process.stdout.write(JSON.stringify({
              event: 'bridge_response',
              ok: false,
              error: { kind: 'set_device_navigate_failed', message: String(navErr) }
            }) + '\n');
            continue;
          }
        }

        const oldContext = context;
        context = newContext;
        page = newPage;
        observationBuffers.clear();
        pages.length = 0;
        pages.push(page);
        await attachObservationListeners(page, pages, 0);
        context.on('page', (p) => {
          if (!pages.includes(p)) {
            pages.push(p);
            const popupIndex = pages.length - 1;
            void attachObservationListeners(p, pages, popupIndex).catch((e) => { process.stderr.write('[acrawl] popup observation attach failed: ' + String(e) + '\n'); });
          }
        });
        await oldContext.close().catch(() => {});

        const title = await page.title().catch(() => '');
        const url = page.url();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: {
            viewport: command.viewport || null,
            userAgent: command.userAgent || null,
            url,
            title
          }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'set_device_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'poll_observations') {
      try {
        const fallbackIndex = pages.indexOf(page);
        const tabIndex = typeof command.tab_index === 'number' ? command.tab_index : fallbackIndex;
        await drainConsolePage(pages[tabIndex], tabIndex);
        const buf = observationBuffers.get(tabIndex);
        const events = buf ? [...buf.events] : [];
        if (buf) {
          buf.events = [];
          buf.currentBytes = 0;
        }
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { events }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'poll_observations_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'set_seq') {
      currentSeq = typeof command.seq === 'number' ? command.seq : 0;
      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: true,
        result: {}
      }) + '\n');
      continue;
    }

    if (command.action === 'start_coverage') {
      try {
        const doJs = command.js !== false;
        const doCss = command.css !== false;
        if (doJs) await pages[activePageIndex()].coverage.startJSCoverage({ resetOnNavigation: false });
        if (doCss) await pages[activePageIndex()].coverage.startCSSCoverage({ resetOnNavigation: false });
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: {}
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'start_coverage_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'stop_coverage') {
      let js_coverage = [];
      let css_coverage = [];
      try { js_coverage = await pages[activePageIndex()].coverage.stopJSCoverage(); } catch(e) {}
      try { css_coverage = await pages[activePageIndex()].coverage.stopCSSCoverage(); } catch(e) {}

      // V8 JS coverage exposes functions[].ranges[]{startOffset,endOffset,count}
      // with NESTED ranges (a function's outer range plus inner block ranges that
      // override the parent's count over their sub-region). Summing every count>0
      // range double-counts overlaps, so sweep the range boundaries with a count
      // stack: between two boundaries the innermost (last-opened) range wins, and
      // its bytes are "used" iff that range's count > 0. This mirrors Playwright's
      // own convertToDisjointRanges. CSS coverage is already a flat disjoint
      // ranges[]{start,end}, so it keeps the simple sum.
      const jsUsedBytes = (functions) => {
        const ranges = [];
        for (const fn of (functions || [])) {
          for (const r of (fn.ranges || [])) {
            if (r.endOffset > r.startOffset) ranges.push(r);
          }
        }
        if (ranges.length === 0) return 0;
        const events = [];
        for (const r of ranges) {
          const len = r.endOffset - r.startOffset;
          events.push({ offset: r.startOffset, isOpen: true, count: r.count, len });
          events.push({ offset: r.endOffset, isOpen: false, count: r.count, len });
        }
        events.sort((a, b) => {
          if (a.offset !== b.offset) return a.offset - b.offset;
          if (a.isOpen !== b.isOpen) return a.isOpen ? 1 : -1;
          return a.isOpen ? b.len - a.len : a.len - b.len;
        });
        const stack = [];
        let lastOffset = 0;
        let used = 0;
        for (const ev of events) {
          if (ev.offset > lastOffset && stack.length > 0 && stack[stack.length - 1] > 0) {
            used += ev.offset - lastOffset;
          }
          lastOffset = ev.offset;
          if (ev.isOpen) {
            stack.push(ev.count);
          } else {
            stack.pop();
          }
        }
        return used;
      };

      const formatEntry = (entry) => {
        let usedBytes;
        if (entry.functions) {
          usedBytes = jsUsedBytes(entry.functions);
        } else if (entry.ranges) {
          usedBytes = entry.ranges.reduce((sum, r) => sum + r.end - r.start, 0);
        } else {
          usedBytes = entry.usedBytes || 0;
        }
        const totalBytes = entry.text ? entry.text.length : (entry.source ? entry.source.length : 0);
        return {
          url: entry.url,
          total_bytes: totalBytes,
          used_bytes: usedBytes,
          unused_bytes: totalBytes - usedBytes,
          unused_pct: totalBytes > 0 ? Math.round((totalBytes - usedBytes) / totalBytes * 1000) / 10 : 0
        };
      };

      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: true,
        result: {
          js_coverage: js_coverage.map(formatEntry),
          css_coverage: css_coverage.map(formatEntry)
        }
      }) + '\n');
      continue;
    }

    if (command.action === 'get_cookies') {
      try {
        const context = browser.contexts()[0];
        const rawCookies = await context.cookies();
        const cookies = rawCookies.map((c) => ({
          name: c.name,
          value: c.value,
          domain: c.domain,
          path: c.path,
          expires: (typeof c.expires === 'number' && c.expires >= 0) ? c.expires : null,
          secure: !!c.secure,
          http_only: !!c.httpOnly,
          same_site: c.sameSite || null,
          size_bytes: (c.name ? c.name.length : 0) + (c.value ? c.value.length : 0),
        }));
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { cookies }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'get_cookies_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'get_storage') {
      try {
        const storageType = command.storage_type || 'all';
        const page = pages[activePageIndex()];

        let localStorage = [];
        let sessionStorage = [];

        if (storageType === 'local' || storageType === 'all') {
          localStorage = await page.evaluate(() => {
            const items = [];
            for (let i = 0; i < window.localStorage.length; i++) {
              const key = window.localStorage.key(i);
              const value = window.localStorage.getItem(key);
              items.push({ key, value, size_bytes: key.length + (value ? value.length : 0) });
            }
            return items;
          });
        }

        if (storageType === 'session' || storageType === 'all') {
          sessionStorage = await page.evaluate(() => {
            const items = [];
            for (let i = 0; i < window.sessionStorage.length; i++) {
              const key = window.sessionStorage.key(i);
              const value = window.sessionStorage.getItem(key);
              items.push({ key, value, size_bytes: key.length + (value ? value.length : 0) });
            }
            return items;
          });
        }

        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { local_storage: localStorage, session_storage: sessionStorage }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'get_storage_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'add_intercept_rule') {
      const { rule_id, pattern, action_type, mock, is_regex } = command;
      const page = pages[activePageIndex()];
      let matcher = pattern;
      if (is_regex) {
        try {
          matcher = new RegExp(pattern);
        } catch (error) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'invalid_regex', message: String(error) } }) + '\n');
          continue;
        }
      }
      interceptRulesMap[rule_id] = { pattern, action_type, mock, hits: 0 };
      await page.route(matcher, async (route) => {
        if (!interceptRulesMap[rule_id]) { await route.continue(); return; }
        interceptRulesMap[rule_id].hits++;
        if (action_type === 'Block') {
          await route.abort();
        } else if (action_type === 'MockResponse' && mock) {
          await route.fulfill({
            status: mock.status || 200,
            contentType: mock.content_type || 'application/json',
            headers: mock.headers || {},
            body: mock.body || '',
          });
        } else {
          await route.continue();
        }
      });
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: {} }) + '\n');
      continue;
    }

    if (command.action === 'remove_intercept_rule') {
      delete interceptRulesMap[command.rule_id];
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: {} }) + '\n');
      continue;
    }

    if (command.action === 'clear_intercept_rules') {
      interceptRulesMap = {};
      await pages[activePageIndex()].unrouteAll();
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: {} }) + '\n');
      continue;
    }

    process.stdout.write(JSON.stringify({
      event: 'bridge_response',
      ok: false,
      error: { kind: 'unsupported_action', message: `Unsupported action: ${command.action}` }
    }) + '\n');
  }
}

bootstrap().catch((error) => {
  process.stdout.write(JSON.stringify({
    event: 'bridge_bootstrap',
    ok: false,
    error: { kind: 'launch_failed', message: String(error) }
  }) + '\n');
  process.exit(1);
});
"#;

#[cfg(test)]
mod tests {
    use super::PLAYWRIGHT_BRIDGE_NODE_SCRIPT;

    #[test]
    fn bridge_fill_has_rich_editor_cascade() {
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("tryRichEditorFill"),
            "Missing tryRichEditorFill function"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("CodeMirror.setValue"),
            "Missing CM5 JS API step"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("execCommand('insertText'"),
            "Missing execCommand contenteditable step"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("keyboard"),
            "Missing keyboard fallback step"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("richEditor"),
            "Missing richEditor field in success payload"
        );
    }

    #[test]
    fn bridge_wraps_coverage_and_storage_handlers_in_try_catch() {
        // start_coverage, get_cookies, and get_storage previously had no
        // try/catch, so an exception inside them (e.g. mid-navigation, or
        // browser.contexts()[0] unavailable during a device-context swap)
        // would propagate out of the `for await` dispatch loop and crash
        // the whole persistent bridge subprocess instead of failing just
        // that one command.
        for (action, error_kind) in [
            ("start_coverage", "start_coverage_failed"),
            ("get_cookies", "get_cookies_failed"),
            ("get_storage", "get_storage_failed"),
        ] {
            let marker = format!("command.action === '{action}'");
            let start = PLAYWRIGHT_BRIDGE_NODE_SCRIPT
                .find(&marker)
                .unwrap_or_else(|| panic!("missing handler for {action}"));
            // The handler body (until the next top-level `if (command.action`)
            // must contain both a try block and the expected error kind.
            let rest = &PLAYWRIGHT_BRIDGE_NODE_SCRIPT[start..];
            let end = rest[marker.len()..]
                .find("if (command.action ===")
                .map_or(rest.len(), |i| i + marker.len());
            let body = &rest[..end];
            assert!(
                body.contains("try {") && body.contains("} catch (error) {"),
                "{action} handler is missing a try/catch wrapper"
            );
            assert!(
                body.contains(error_kind),
                "{action} handler is missing expected error kind {error_kind}"
            );
        }
    }

    #[test]
    fn bridge_has_editor_surface_redirect() {
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("resolveEditorSurface"),
            "Missing resolveEditorSurface function"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains("getBoundingClientRect"),
            "Missing visibility check in proximity walk"
        );
        assert!(
            PLAYWRIGHT_BRIDGE_NODE_SCRIPT.contains(r#"[contenteditable="true"], textarea"#),
            "Missing contenteditable candidate search in proximity walk"
        );
    }
}
