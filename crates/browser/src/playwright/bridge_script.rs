#[allow(clippy::needless_raw_string_hashes)]
pub(crate) const PLAYWRIGHT_BRIDGE_NODE_SCRIPT: &str = r#"
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

const observationCdpSessions = new WeakMap();

function truncateText(value, maxChars) {
  if (maxChars === undefined) maxChars = 8192;
  const text = (typeof value === 'string') ? value : String(value === null || value === undefined ? '' : value);
  return text.length > maxChars ? text.slice(0, maxChars) + '...[truncated]' : text;
}

function normalizeConsoleLevel(type) {
  switch (String(type || '').toLowerCase()) {
    case 'error':
    case 'assert':
      return 'error';
    case 'warn':
    case 'warning':
      return 'warning';
    case 'debug':
    case 'trace':
      return 'debug';
    default:
      return 'info';
  }
}

function toNullableInt(value) {
  return Number.isInteger(value) ? value : null;
}

function firstNonEmpty() {
  for (let i = 0; i < arguments.length; i++) {
    const value = arguments[i];
    if (typeof value === 'string' && value.trim() !== '') return value;
  }
  return null;
}

function topCallFrame(stackTrace) {
  const frames = (stackTrace && Array.isArray(stackTrace.callFrames)) ? stackTrace.callFrames : [];
  return frames.length > 0 ? frames[0] : null;
}

function formatPreview(obj, preview) {
  const props = Array.isArray(preview.properties) ? preview.properties.slice(0, 5) : [];
  const rendered = props.map((prop) => {
    if (prop.value !== undefined) {
      return prop.name + ': ' + (prop.type === 'string' ? JSON.stringify(prop.value) : String(prop.value));
    }
    return prop.name + ': ' + (prop.subtype || prop.type || '...');
  });
  const overflow = (preview.overflow || (Array.isArray(preview.properties) && preview.properties.length > 5)) ? ', ...' : '';
  if (preview.subtype === 'array') return '[' + rendered.join(', ') + overflow + ']';
  if (rendered.length > 0) return (obj.className || 'Object') + ' { ' + rendered.join(', ') + overflow + ' }';
  return obj.description || obj.className || 'Object';
}

function formatRemoteObject(obj) {
  if (!obj || typeof obj !== 'object') return '';
  if (obj.unserializableValue !== undefined) return String(obj.unserializableValue);
  if (obj.subtype === 'null') return 'null';
  if (obj.type === 'undefined') return 'undefined';
  if (obj.type === 'string') return (obj.value !== undefined ? obj.value : (obj.description || ''));
  if (obj.type === 'number' || obj.type === 'boolean' || obj.type === 'bigint') {
    return obj.value !== undefined ? String(obj.value) : String(obj.description || '');
  }
  if (obj.type === 'function') return obj.description || '[Function]';
  if (obj.preview) return formatPreview(obj, obj.preview);
  return obj.description || obj.className || obj.type || 'Object';
}

function formatConsoleArgs(args) {
  const parts = Array.isArray(args) ? args.map(formatRemoteObject) : [];
  return truncateText(parts.filter((p) => p !== '').join(' '));
}

function attachLegacyConsoleListeners(page, pageIndex) {
  page.on('console', (msg) => {
    const location = typeof msg.location === 'function' ? msg.location() : null;
    bufferEvent(pageIndex, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      level: msg.type(),
      message_type: 'Console',
      text: truncateText(msg.text()),
      source_url: location && location.url ? location.url : null,
      source_line: location && location.lineNumber != null ? location.lineNumber : null,
      source_column: location && location.columnNumber != null ? location.columnNumber : null,
      stack: null,
    });
  });
  page.on('pageerror', (err) => {
    bufferEvent(pageIndex, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      level: 'error',
      message_type: 'Exception',
      text: truncateText(err.message),
      source_url: null,
      source_line: null,
      source_column: null,
      stack: err.stack ? truncateText(err.stack, 16384) : null,
    });
  });
}

async function detachObservationSession(page) {
  const client = observationCdpSessions.get(page);
  observationCdpSessions.delete(page);
  if (!client) return;
  try { client.removeAllListeners(); } catch (_) {}
  await client.detach().catch(() => {});
}

// CloakBrowser (rebrowser-style stealth) suppresses CDP Runtime.enable, so Playwright's
// page.on('console')/'pageerror' never fire for page-origin JS. A dedicated CDP session
// re-enables Runtime on a separate target to capture console calls + uncaught exceptions.
async function initCdpObservation(page, pageIndex) {
  const client = await page.context().newCDPSession(page);
  observationCdpSessions.set(page, client);

  client.on('Runtime.consoleAPICalled', (event) => {
    const frame = topCallFrame(event && event.stackTrace);
    bufferEvent(pageIndex, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      level: normalizeConsoleLevel(event && event.type),
      message_type: 'Console',
      text: formatConsoleArgs(event && event.args) || String((event && event.type) || 'console'),
      source_url: frame && frame.url ? frame.url : null,
      source_line: frame ? toNullableInt(frame.lineNumber) : null,
      source_column: frame ? toNullableInt(frame.columnNumber) : null,
      stack: null,
    });
  });

  client.on('Runtime.exceptionThrown', (event) => {
    const details = (event && event.exceptionDetails) || {};
    const exception = details.exception || {};
    const frame = topCallFrame(details.stackTrace);
    bufferEvent(pageIndex, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: pageIndex,
      level: 'error',
      message_type: 'Exception',
      text: truncateText(firstNonEmpty(details.text, typeof exception.value === 'string' ? exception.value : null, 'Uncaught exception')),
      source_url: (frame && frame.url) ? frame.url : (details.url || null),
      source_line: frame ? toNullableInt(frame.lineNumber) : toNullableInt(details.lineNumber),
      source_column: frame ? toNullableInt(frame.columnNumber) : toNullableInt(details.columnNumber),
      stack: (typeof exception.description === 'string' && exception.description) ? truncateText(exception.description, 16384) : null,
    });
  });

  await client.send('Runtime.enable');
}

async function attachObservationListeners(page, pageIndex) {
  if (page.__acrawlObservationAttached) {
    return page.__acrawlObservationAttachPromise;
  }
  page.__acrawlObservationAttached = true;

  const pendingRequests = new WeakMap();

  page.on('request', (req) => {
    if (req.url().includes('__acrawl_poll')) return;

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
    }, tracked?.seqAtInitiation ?? currentSeq);

    pendingRequests.delete(req);
  });

  page.on('requestfailed', (req) => {
    if (req.url().includes('__acrawl_poll')) return;

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
    const wsId = `ws_${pageIndex}_${++nextWebSocketId}`;

    ws.on('framesent', (frame) => {
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

  page.once('close', () => { void detachObservationSession(page); });

  page.__acrawlObservationAttachPromise = (async () => {
    try {
      await initCdpObservation(page, pageIndex);
    } catch (error) {
      process.stderr.write('[acrawl] CDP observation attach failed on tab ' + pageIndex + ': ' + String(error) + '\n');
      attachLegacyConsoleListeners(page, pageIndex);
    }
  })();
  return page.__acrawlObservationAttachPromise;
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
  let page = await context.newPage();
  const pages = [page];
  await attachObservationListeners(page, 0);
  context.on('page', (p) => {
    if (!pages.includes(p)) {
      pages.push(p);
      const popupIndex = pages.length - 1;
      void attachObservationListeners(p, popupIndex).catch((e) => { process.stderr.write('[acrawl] popup observation attach failed: ' + String(e) + '\n'); });
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
        await attachObservationListeners(newPage, pageIndex);
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
        pages[pageIndex] = null;
        observationBuffers.delete(pageIndex);
        if (page === targetPage) {
          const fallbackPage = pages.find((entry) => entry);
          if (fallbackPage) {
            page = fallbackPage;
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
      try {
        const sel = await resolveFillSelector(page, command.selector);
        await page.fill(sel, command.value, { timeout: 5000 });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, resolvedSelector: sel } }) + '\n');
      } catch (mainError) {
        let filledInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.fill(command.selector, command.value, { timeout: 2000 });
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
        const compoundEnrichment = command.compoundEnrichment || false;
        const result = await page.evaluate(({scope, compoundEnrichment}) => {
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

          function getEnrichment(el, tag, elType) {
            if (!compoundEnrichment) return null;
            let enrichment = null;
            if (tag === 'input') {
              const inputType = elType || el.type || '';
              if (inputType === 'date') enrichment = { format: 'YYYY-MM-DD' };
              else if (inputType === 'time') enrichment = { format: 'HH:MM' };
              else if (inputType === 'datetime-local') enrichment = { format: 'YYYY-MM-DDTHH:MM' };
              else if (inputType === 'range') {
                enrichment = {
                  min: el.min !== '' ? Number(el.min) : 0,
                  max: el.max !== '' ? Number(el.max) : 100,
                  step: el.step !== '' ? Number(el.step) : 1,
                  value: el.value !== '' ? Number(el.value) : 50
                };
              } else if (inputType === 'number') {
                enrichment = {};
                if (el.min !== '') enrichment.min = Number(el.min);
                if (el.max !== '') enrichment.max = Number(el.max);
                if (el.step !== '') enrichment.step = Number(el.step);
                if (Object.keys(enrichment).length === 0) enrichment = null;
              } else if (inputType === 'color') {
                enrichment = { value: el.value || '#000000' };
              } else if (inputType === 'file') {
                enrichment = { accept: el.accept || '*' };
              }
            } else if (tag === 'select') {
              const opts = Array.from(el.options || []);
              const total = opts.length;
              const visible = opts.slice(0, 20).map(o => o.text.trim()).filter(t => t.length > 0);
              if (total > 20) visible.push('...and ' + (total - 20) + ' more');
              enrichment = { options: visible, total_options: total };
            } else if (tag === 'textarea') {
              const ml = el.getAttribute('maxlength');
              if (ml) enrichment = { maxlength: Number(ml) };
            }
            if (enrichment) {
              const json = JSON.stringify(enrichment);
              if (json.length > 200) {
                if (enrichment.options && Array.isArray(enrichment.options)) {
                  while (JSON.stringify(enrichment).length > 190 && enrichment.options.length > 1) {
                    enrichment.options.pop();
                  }
                } else {
                  return null;
                }
              }
            }
            return enrichment;
          }

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
                const enrichment = getEnrichment(el, entry.tag, entry.type);
                if (enrichment !== null) entry.enrichment = enrichment;
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
        }, {scope, compoundEnrichment});
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'page_map_error', message: String(error) } }) + '\n');
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
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'select_option_failed', message: String(error) } }) + '\n');
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
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'hover_failed', message: String(error) } }) + '\n');
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
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'press_key_failed', message: String(error) } }) + '\n');
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
        const response = await context.request.get(command.url, { timeout: 30000 });
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
        await attachObservationListeners(page, 0);
        context.on('page', (p) => {
          if (!pages.includes(p)) {
            pages.push(p);
            const popupIndex = pages.length - 1;
            void attachObservationListeners(p, popupIndex).catch((e) => { process.stderr.write('[acrawl] popup observation attach failed: ' + String(e) + '\n'); });
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
      const doJs = command.js !== false;
      const doCss = command.css !== false;
      if (doJs) await pages[activePageIndex()].coverage.startJSCoverage({ resetOnNavigation: false });
      if (doCss) await pages[activePageIndex()].coverage.startCSSCoverage({ resetOnNavigation: false });
      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: true,
        result: {}
      }) + '\n');
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
      continue;
    }

    if (command.action === 'get_storage') {
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
