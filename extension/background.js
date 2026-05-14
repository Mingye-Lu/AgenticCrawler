'use strict';

// Connection state
let ws = null;
let wsConnected = false;
let reconnectTimer = null;
let reconnectDelay = 1000; // exponential backoff, cap at 30000ms
let keepaliveInterval = null;

// Tab management: maps pageIndex -> Chrome tabId
const managedTabs = {};
let nextPageIndex = 0;

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
  // Placeholder — will be implemented in T5-T9
  const response = {
    id: cmd.id,
    ok: false,
    error: 'not implemented'
  };
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(response));
  }
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
  connect();
});

chrome.runtime.onInstalled.addListener(() => {
  connect();
});

// Connect on service worker activation
connect();
