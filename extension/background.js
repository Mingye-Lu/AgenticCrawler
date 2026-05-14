'use strict';

try {
  importScripts('commands/cdp.js', 'commands/navigate.js', 'commands/click.js', 'commands/fill.js');
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

// Pending commands: requestId -> {resolve, reject, timeoutId}
const pendingRequests = {};
let nextRequestId = 1;

// ----------- Connection management -----------

async function getSettings() {
  return new Promise((resolve) => {
    chrome.storage.local.get({ port: 19876, token: '' }, resolve);
  });
}

async function connect() {
  const { port, token } = await getSettings();
  if (!token) {
    setBadge('?', '#888888');
    return;
  }

  try {
    ws = new WebSocket(`ws://127.0.0.1:${port}/bridge?token=${encodeURIComponent(token)}`);

    ws.onopen = () => {
      wsConnected = true;
      reconnectDelay = 1000;
      setBadge('', '#00aa00'); // green
      startKeepalive();
    };

    ws.onmessage = (event) => {
      handleMessage(event.data);
    };

    ws.onclose = () => {
      wsConnected = false;
      ws = null;
      setBadge('', '#cc0000'); // red
      stopKeepalive();
      scheduleReconnect();
    };

    ws.onerror = () => {
      // onclose will fire after onerror
    };
  } catch (e) {
    scheduleReconnect();
  }
}

function disconnect() {
  if (ws) {
    ws.close();
    ws = null;
  }
  clearReconnectTimer();
  stopKeepalive();
  wsConnected = false;
  setBadge('', '#cc0000');
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

      // Commands requiring an active tab
      default: {
        const tabId = getActiveTabId();
        if (!tabId) {
          sendResponse(cmd.id, false, null, 'No active tab. Use new_page to open a tab first.');
          return;
        }
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
          case 'fill':
            result = await handleFill(tabId, cmd.payload || {});
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
  connect();
});

chrome.runtime.onInstalled.addListener(() => {
  connect();
});

// Connect on service worker activation
connect();
