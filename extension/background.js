'use strict';

try {
  importScripts(
    'commands/cdp.js',
    'commands/navigate.js',
    'commands/click.js',
    'commands/fill.js',
    'commands/screenshot.js',
    'commands/execute_js.js',
    'commands/page_map.js',
    'commands/extract_dom_snapshot.js',
    'commands/read_content.js',
    'commands/list_resources.js',
    'commands/hover.js',
    'commands/press_key.js',
    'commands/scroll.js',
    'commands/wait.js',
    'commands/select_option.js',
    'commands/save_file.js',
    'commands/click_at.js',
    'commands/cookies.js',
    'commands/set_device.js',
    'commands/poll_observations.js'
  );
} catch (e) {
  console.error('Failed to import command scripts:', e);
}

// Connection state
let ws = null;
let wsConnected = false;
let reconnectTimer = null;
let reconnectDelay = 1000; // exponential backoff, cap at 30000ms
let keepaliveInterval = null;

// Tab management: maps pageIndex -> Chrome tabId
const managedTabs = {};
let nextPageIndex = 0;
let activePageIndex = 0;

// Observation buffers: tabId -> { events: [], currentBytes: 0 }
const observationBuffers = new Map();
const observationRequestMetadata = new Map();
const observationEnabledTabs = new Set();
const MAX_OBSERVATION_BYTES = 2 * 1024 * 1024; // 2MB
let currentObservationSeq = 0;

function estimateEventBytes(event) {
  return JSON.stringify(event).length;
}

function getTabIndexForObservation(tabId) {
  const entry = Object.entries(managedTabs).find(([, id]) => id === tabId);
  return entry ? Number(entry[0]) : 0;
}

function getObservationMetadataStore(tabId) {
  let store = observationRequestMetadata.get(tabId);
  if (!store) {
    store = new Map();
    observationRequestMetadata.set(tabId, store);
  }
  return store;
}

function bufferObservationEvent(tabId, event) {
  event.seq_at_initiation = currentObservationSeq;

  if (!observationBuffers.has(tabId)) {
    observationBuffers.set(tabId, { events: [], currentBytes: 0 });
  }
  const buf = observationBuffers.get(tabId);
  const eventBytes = estimateEventBytes(event);

  buf.events.push(event);
  buf.currentBytes += eventBytes;

  // Evict oldest while over cap
  while (buf.currentBytes > MAX_OBSERVATION_BYTES && buf.events.length > 0) {
    const removed = buf.events.shift();
    buf.currentBytes -= estimateEventBytes(removed);
  }
}

function drainObservationBuffer(tabId) {
  const buf = observationBuffers.get(tabId);
  const events = buf ? [...buf.events] : [];
  if (buf) {
    buf.events = [];
    buf.currentBytes = 0;
  }
  return events;
}

function clearObservationState(tabId) {
  observationBuffers.delete(tabId);
  observationRequestMetadata.delete(tabId);
  observationEnabledTabs.delete(tabId);
}

async function ensureObservationEnabled(tabId) {
  await ensureAttached(tabId);
  if (observationEnabledTabs.has(tabId)) {
    return;
  }

  // Enable CDP domains for observation
  await cdp(tabId, 'Network.enable', {});
  await cdp(tabId, 'Runtime.enable', {});
  await cdp(tabId, 'Page.enable', {});
  observationEnabledTabs.add(tabId);
}

chrome.debugger.onEvent.addListener((source, method, params) => {
  const tabId = source.tabId;
  if (!tabId || !observationEnabledTabs.has(tabId)) {
    return;
  }

  const tabIndex = getTabIndexForObservation(tabId);
  const metadataStore = getObservationMetadataStore(tabId);

  if (method === 'Network.requestWillBeSent') {
    metadataStore.set(params.requestId, {
      url: params.request?.url || '',
      method: params.request?.method || '',
      request_type: params.type || 'other',
      initiator_type: params.initiator?.type || null,
      from_service_worker: params.initiator?.type === 'script' && params.initiator?.url?.includes('sw.js'),
      started_at: Date.now(),
    });

    bufferObservationEvent(tabId, {
      type: 'NetworkRequest',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      request_id: params.requestId,
      url: params.request?.url || '',
      method: params.request?.method || '',
      status: null,
      state: 'Pending',
      size_bytes: null,
      duration_ms: null,
      request_type: params.type || 'other',
      from_service_worker: params.initiator?.type === 'script' && params.initiator?.url?.includes('sw.js'),
      initiator_type: params.initiator?.type || null,
      reason: null,
    });
    return;
  }

  if (method === 'Network.responseReceived') {
    const metadata = metadataStore.get(params.requestId);
    bufferObservationEvent(tabId, {
      type: 'NetworkRequest',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      request_id: `${params.requestId}_response`,
      url: params.response?.url || metadata?.url || '',
      method: metadata?.method || 'GET',
      status: params.response?.status ?? null,
      state: 'Completed',
      size_bytes: params.response?.encodedDataLength || null,
      duration_ms: metadata?.started_at ? Math.max(0, Date.now() - metadata.started_at) : null,
      request_type: params.type || metadata?.request_type || 'other',
      from_service_worker: metadata?.from_service_worker || false,
      initiator_type: metadata?.initiator_type || null,
      reason: null,
    });
    metadataStore.delete(params.requestId);
    return;
  }

  if (method === 'Network.loadingFailed') {
    const metadata = metadataStore.get(params.requestId);
    bufferObservationEvent(tabId, {
      type: 'NetworkRequest',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      request_id: `${params.requestId}_failed`,
      url: metadata?.url || '',
      method: metadata?.method || '',
      status: null,
      state: params.canceled ? 'Aborted' : 'Failed',
      size_bytes: null,
      duration_ms: metadata?.started_at ? Math.max(0, Date.now() - metadata.started_at) : null,
      request_type: params.type || metadata?.request_type || 'other',
      from_service_worker: metadata?.from_service_worker || false,
      initiator_type: metadata?.initiator_type || null,
      reason: params.errorText || 'Unknown',
    });
    metadataStore.delete(params.requestId);
    return;
  }

  if (method === 'Runtime.consoleAPICalled') {
    const text = params.args?.map((arg) => arg.value ?? arg.description ?? '').join(' ') || '';
    bufferObservationEvent(tabId, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      level: params.type,
      message_type: 'Console',
      text,
      source_url: params.stackTrace?.callFrames?.[0]?.url || null,
      source_line: params.stackTrace?.callFrames?.[0]?.lineNumber || null,
      source_column: params.stackTrace?.callFrames?.[0]?.columnNumber || null,
      stack: null,
    });
    return;
  }

  if (method === 'Runtime.exceptionThrown') {
    const details = params.exceptionDetails || {};
    bufferObservationEvent(tabId, {
      type: 'ConsoleMessage',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      level: 'error',
      message_type: 'Exception',
      text: details.exception?.description || details.text || 'Unknown error',
      source_url: details.url || null,
      source_line: details.lineNumber || null,
      source_column: details.columnNumber || null,
      stack: details.stackTrace?.callFrames?.map((frame) => `  at ${frame.functionName} (${frame.url}:${frame.lineNumber})`).join('\n') || null,
    });
    return;
  }

  if (method === 'Network.webSocketFrameReceived') {
    const metadata = metadataStore.get(params.requestId);
    bufferObservationEvent(tabId, {
      type: 'WebSocketFrame',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      connection_id: params.requestId,
      url: metadata?.url || '',
      direction: 'received',
      data: params.response?.payloadData || '',
      size_bytes: params.response?.payloadData?.length || 0,
      connection_status: 'open',
    });
    return;
  }

  if (method === 'Network.webSocketFrameSent') {
    const metadata = metadataStore.get(params.requestId);
    bufferObservationEvent(tabId, {
      type: 'WebSocketFrame',
      timestamp_ms: Date.now(),
      tab_index: tabIndex,
      connection_id: params.requestId,
      url: metadata?.url || '',
      direction: 'sent',
      data: params.response?.payloadData || '',
      size_bytes: params.response?.payloadData?.length || 0,
      connection_status: 'open',
    });
  }
});

// ----------- Connection management -----------

async function getSettings() {
  return new Promise((resolve) => {
    chrome.storage.local.get({ port: 19876, token: '' }, resolve);
  });
}

async function getConnectionStatus() {
  const { token } = await getSettings();
  return {
    connected: wsConnected,
    connecting: Boolean(ws && ws.readyState === WebSocket.CONNECTING),
    configured: Boolean(token),
  };
}

async function connect(timeoutMs = 0) {
  const { port, token } = await getSettings();

  if (!token) {
    disconnect({ suppressReconnect: true, unconfigured: true });
    return false;
  }

  clearReconnectTimer();
  stopKeepalive();

  if (ws) {
    disconnect({ suppressReconnect: true, preserveBadge: true });
  }

  return new Promise((resolve) => {
    let settled = false;
    let timeoutId = null;
    const socket = new WebSocket(`ws://127.0.0.1:${port}/bridge?token=${encodeURIComponent(token)}`);
    ws = socket;
    wsConnected = false;
    setBadge('...', '#888888');

    const finish = (connected) => {
      if (settled) {
        return;
      }
      settled = true;
      if (timeoutId !== null) {
        clearTimeout(timeoutId);
      }
      resolve(connected);
    };

    if (timeoutMs > 0) {
      timeoutId = setTimeout(() => {
        if (ws === socket) {
          disconnect({ suppressReconnect: true });
        }
        finish(false);
      }, timeoutMs);
    }

    socket.onopen = () => {
      if (ws !== socket) {
        socket.acrawlManualClose = true;
        socket.close();
        finish(false);
        return;
      }
      wsConnected = true;
      reconnectDelay = 1000;
      setBadge('', '#00aa00');
      startKeepalive();
      finish(true);
    };

    socket.onmessage = (event) => {
      handleMessage(event.data);
    };

    socket.onclose = () => {
      const isCurrentSocket = ws === socket;
      const wasManualClose = socket.acrawlManualClose === true;
      if (isCurrentSocket) {
        ws = null;
        wsConnected = false;
        stopKeepalive();
        setBadge('', '#cc0000');
        if (!wasManualClose) {
          scheduleReconnect();
        }
      }
      finish(false);
    };

    socket.onerror = () => {
      // onclose will fire after onerror
    };
  });
}

function disconnect(options = {}) {
  const { suppressReconnect = true, preserveBadge = false, unconfigured = false } = options;
  const socket = ws;
  ws = null;
  wsConnected = false;
  clearReconnectTimer();
  stopKeepalive();
  if (socket) {
    socket.acrawlManualClose = suppressReconnect;
    socket.close();
  }
  if (unconfigured) {
    setBadge('?', '#888888');
  } else if (!preserveBadge) {
    setBadge('', '#cc0000');
  }
}

function scheduleReconnect() {
  clearReconnectTimer();
  reconnectTimer = setTimeout(() => {
    connect();
  }, reconnectDelay);
  reconnectDelay = Math.min(reconnectDelay * 2, 30000);
}

function clearReconnectTimer() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

async function reconnect() {
  return connect(5000);
}

function startKeepalive() {
  stopKeepalive();
  keepaliveInterval = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: 'ping' }));
    }
  }, 20000);
}

function stopKeepalive() {
  if (keepaliveInterval) {
    clearInterval(keepaliveInterval);
    keepaliveInterval = null;
  }
}

// ----------- Badge -----------

function setBadge(text, color) {
  chrome.action.setBadgeText({ text });
  chrome.action.setBadgeBackgroundColor({ color });
}

// ----------- Extension messages -----------

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message && message.type === 'getConnectionStatus') {
    getConnectionStatus()
      .then(sendResponse)
      .catch(() => sendResponse({ connected: false, connecting: false, configured: false }));
    return true;
  }

  if (message && message.type === 'reconnect') {
    reconnect()
      .then((connected) => sendResponse({ ok: connected, connected }))
      .catch((error) => sendResponse({ ok: false, error: error && error.message ? error.message : String(error) }));
    return true;
  }

  return false;
});

// ----------- Message handler -----------

function handleMessage(data) {
  try {
    const msg = JSON.parse(data);

    // Keepalive pong
    if (msg.type === 'pong') return;

    // Command from acrawl → dispatch to handler
    if (msg.action) {
      handleCommand(msg);
      return;
    }
  } catch (e) {
    // ignore parse errors
  }
}

async function handleCommand(cmd) {
  try {
    let result;
    switch (cmd.action) {
      // Tab lifecycle (no active tab required)
      case 'new_page':
        result = await handleNewPage(cmd.payload || {});
        break;
      case 'close_page':
        result = await handleClosePage(cmd.payload || {});
        break;
      case 'switch_tab':
        result = await handleSwitchTab(cmd.payload || {});
        break;
      case 'close':
        result = await handleClose();
        break;
      case 'set_seq':
        currentObservationSeq = Number.isFinite(cmd.payload?.seq) ? cmd.payload.seq : 0;
        result = {};
        break;

      // Commands requiring an active tab
      default: {
        const tabId = getActiveTabId();
        if (!tabId) {
          sendResponse(cmd.id, false, null, 'No active tab. Use new_page to open a tab first.');
          return;
        }
        await ensureObservationEnabled(tabId);
        switch (cmd.action) {
          case 'navigate':
            result = await handleNavigate(tabId, cmd.payload || {});
            break;
          case 'go_back':
            result = await handleGoBack(tabId, cmd.payload || {});
            break;
          case 'click':
            result = await handleClick(tabId, cmd.payload || {});
            break;
          case 'click_at':
            result = await handleClickAt(tabId, cmd.payload || {});
            break;
          case 'fill':
            result = await handleFill(tabId, cmd.payload || {});
            break;
          case 'screenshot':
            result = await handleScreenshot(tabId, cmd.payload || {});
            break;
          case 'execute_js':
            result = await handleExecuteJs(tabId, cmd.payload || {});
            break;
          case 'page_map':
            result = await handlePageMap(tabId, cmd.payload || {});
            break;
          case 'extract_dom_snapshot':
            result = await handleExtractDomSnapshot(tabId, cmd.payload || {});
            break;
          case 'read_content':
            result = await handleReadContent(tabId, cmd.payload || {});
            break;
          case 'list_resources':
            result = await handleListResources(tabId);
            break;
          case 'hover':
            result = await handleHover(tabId, cmd.payload || {});
            break;
          case 'press_key':
            result = await handlePressKey(tabId, cmd.payload || {});
            break;
          case 'scroll':
            result = await handleScroll(tabId, cmd.payload || {});
            break;
          case 'wait_for_selector':
            result = await handleWait(tabId, cmd.payload || {});
            break;
          case 'select_option':
            result = await handleSelectOption(tabId, cmd.payload || {});
            break;
          case 'save_file':
            result = await handleSaveFile(tabId, cmd.payload || {});
            break;
          case 'export_cookies':
            result = await handleExportCookies(tabId);
            break;
          case 'import_cookies':
            result = await handleImportCookies(tabId, cmd.payload || {});
            break;
          case 'import_cookies_only':
            result = await handleImportCookiesOnly(tabId, cmd.payload || {});
            break;
          case 'import_local_storage':
            result = await handleImportLocalStorage(tabId, cmd.payload || {});
            break;
          case 'set_device':
            result = await handleSetDevice(tabId, cmd.payload || {});
            break;
          case 'poll_observations':
            result = await handlePollObservations(tabId, cmd.payload || {});
            break;
          default:
            sendResponse(cmd.id, false, null, `Unknown action: ${cmd.action}`);
            return;
        }
      }
    }
    sendResponse(cmd.id, true, result, null);
  } catch (e) {
    sendResponse(cmd.id, false, null, e && e.message ? e.message : String(e));
  }
}

function sendResponse(id, ok, result, error) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    const msg = { id, ok };
    if (result !== null) {
      msg.result = result;
    }
    if (error !== null) {
      msg.error = error;
    }
    ws.send(JSON.stringify(msg));
  }
}

function getActiveTabId() {
  if (Number.isInteger(activePageIndex) && managedTabs[activePageIndex]) {
    return managedTabs[activePageIndex];
  }

  const indices = Object.keys(managedTabs)
    .map(Number)
    .sort((a, b) => a - b);

  return indices.length > 0 ? managedTabs[indices[indices.length - 1]] : null;
}

// ----------- Tab management -----------

async function handleNewPage(payload) {
  const url = payload.url || 'about:blank';
  const tab = await new Promise((resolve, reject) => {
    chrome.tabs.create({ url, active: false }, (t) => {
      if (chrome.runtime.lastError) reject(new Error(chrome.runtime.lastError.message));
      else resolve(t);
    });
  });
  const pageIndex = nextPageIndex++;
  managedTabs[pageIndex] = tab.id;
  activePageIndex = pageIndex;
  await saveState();
  return { pageIndex };
}

async function handleClosePage(payload) {
  const pageIndex = payload.page_index ?? activePageIndex;
  const tabId = managedTabs[pageIndex];
  if (tabId) {
    await detachDebugger(tabId);
    clearObservationState(tabId);
    await new Promise((resolve) => chrome.tabs.remove(tabId, resolve));
    delete managedTabs[pageIndex];
    await saveState();
  }
  return { closed: true };
}

async function handleSwitchTab(payload) {
  const index = payload.index ?? 0;
  const tabId = managedTabs[index];
  if (!tabId) throw new Error(`No tab at index ${index}`);
  await new Promise((resolve, reject) => {
    chrome.tabs.update(tabId, { active: true }, (t) => {
      if (chrome.runtime.lastError) reject(new Error(chrome.runtime.lastError.message));
      else resolve(t);
    });
  });
  activePageIndex = index;
  await saveState();
  return { switched: true, pageIndex: index };
}

async function handleClose() {
  const tabIds = Object.values(managedTabs);
  for (const tabId of tabIds) {
    await detachDebugger(tabId).catch(() => {});
    clearObservationState(tabId);
  }
  for (const tabId of tabIds) {
    await new Promise((resolve) => chrome.tabs.remove(tabId, resolve)).catch(() => {});
  }
  Object.keys(managedTabs).forEach((k) => delete managedTabs[k]);
  nextPageIndex = 0;
  activePageIndex = 0;
  await saveState();
  if (ws) ws.close();
  return { closed: true };
}

async function detachDebugger(tabId) {
  await new Promise((resolve) => {
    chrome.debugger.detach({ tabId }, () => {
      if (chrome.runtime.lastError) { /* already detached */ }
      resolve();
    });
  });
}

// ----------- Tab lifecycle events -----------

chrome.tabs.onRemoved.addListener((tabId) => {
  const entry = Object.entries(managedTabs).find(([, id]) => id === tabId);
  if (entry) {
    const [pageIndex] = entry;
    clearObservationState(tabId);
    delete managedTabs[Number(pageIndex)];
    saveState();
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({
        type: 'tab_closed',
        pageIndex: Number(pageIndex),
        error: 'Tab was closed externally by user',
      }));
    }
  }
});

chrome.debugger.onDetach.addListener((source, reason) => {
  const tabId = source.tabId;
  clearObservationState(tabId);
  const entry = Object.entries(managedTabs).find(([, id]) => id === tabId);
  if (entry) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({
        type: 'debugger_detached',
        tabId,
        reason,
        error: 'Browser debugger was dismissed. Run /extension to re-attach.',
      }));
    }
  }
});

// ----------- State persistence -----------

async function saveState() {
  await new Promise((resolve) => {
    chrome.storage.session.set({
      managedTabs: JSON.parse(JSON.stringify(managedTabs)),
      nextPageIndex,
      activePageIndex,
    }, resolve);
  });
}

async function loadState() {
  return new Promise((resolve) => {
    chrome.storage.session.get(
      { managedTabs: {}, nextPageIndex: 0, activePageIndex: 0 },
      resolve
    );
  });
}

async function restoreState() {
  const state = await loadState();
  Object.assign(managedTabs, state.managedTabs);
  nextPageIndex = state.nextPageIndex;
  activePageIndex = state.activePageIndex;
}

// ----------- Alarms watchdog -----------

chrome.alarms.create('ws-watchdog', { periodInMinutes: 0.5 });

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === 'ws-watchdog') {
    if (!wsConnected && (!ws || ws.readyState === WebSocket.CLOSED)) {
      connect();
    }
  }
});

// ----------- Startup -----------

chrome.runtime.onStartup.addListener(() => {
  restoreState().then(connect);
});

chrome.runtime.onInstalled.addListener(() => {
  connect();
});

(async () => {
  await restoreState();
  await connect();
})();
