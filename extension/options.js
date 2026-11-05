'use strict';

document.getElementById('version').textContent = 'v' + chrome.runtime.getManifest().version;
const portInput = document.getElementById('port');
const saveButton = document.getElementById('save');
const testButton = document.getElementById('test');
const statusSpan = document.getElementById('status');
const powerButton = document.getElementById('power');
let bridgeEnabled = true;
let statusTimer = null;

function renderPower(enabled) {
  bridgeEnabled = enabled;
  powerButton.disabled = false;
  powerButton.textContent = enabled ? 'Stop' : 'Start';
  powerButton.className = enabled ? '' : 'start';
  powerButton.title = enabled
    ? 'Stop connecting to acrawl'
    : 'Start connecting to acrawl';
}

function showStopped() {
  statusSpan.textContent = 'Stopped: not connecting';
  statusSpan.className = 'stopped';
}

// Start / stop the bridge. While stopped the extension makes no connection attempts.
powerButton.addEventListener('click', () => {
  const enable = !bridgeEnabled;
  powerButton.disabled = true;
  clearTimeout(statusTimer);
  if (enable) {
    statusSpan.textContent = 'Connecting...';
    statusSpan.className = 'testing';
  }
  chrome.runtime.sendMessage({ type: 'setEnabled', enabled: enable }, (response) => {
    if (chrome.runtime.lastError || !response || !response.ok) {
      powerButton.disabled = false;
      statusSpan.textContent = enable ? 'Could not start' : 'Could not stop';
      statusSpan.className = 'disconnected';
      return;
    }
    renderPower(response.enabled);
    updateStatusDisplay();
  });
});

// Display toggles persist immediately; the service worker picks them up via storage.onChanged.
const toggleKeys = ['showIndicators', 'highlightChanges'];
chrome.storage.local.get(Object.fromEntries(toggleKeys.map((key) => [key, true])), (items) => {
  for (const key of toggleKeys) {
    const box = document.getElementById(key);
    box.checked = items[key] !== false;
    box.addEventListener('change', () => chrome.storage.local.set({ [key]: box.checked }));
  }
});

// Load settings on page load
document.addEventListener('DOMContentLoaded', () => {
  chrome.storage.local.get({ port: 19876 }, (items) => {
    portInput.value = items.port;
    updateStatusDisplay();
  });
});

// Save settings
saveButton.addEventListener('click', () => {
  const port = parseInt(portInput.value, 10);

  if (isNaN(port) || port < 1 || port > 65535) {
    statusSpan.textContent = 'Invalid port number';
    statusSpan.className = 'disconnected';
    return;
  }

  chrome.storage.local.set({ port }, () => {
    statusSpan.textContent = 'Connecting...';
    statusSpan.className = 'testing';
    saveButton.disabled = true;
    testButton.disabled = true;
    if (!bridgeEnabled) {
      saveButton.disabled = false;
      testButton.disabled = false;
      statusSpan.textContent = 'Saved. Press Start to connect';
      statusSpan.className = 'stopped';
      return;
    }
    chrome.runtime.sendMessage({ type: 'reconnect' }, (response) => {
      saveButton.disabled = false;
      testButton.disabled = false;
      if (chrome.runtime.lastError) {
        statusSpan.textContent = 'Connection failed';
        statusSpan.className = 'disconnected';
        return;
      }
      if (response && response.connected) {
        statusSpan.textContent = 'Connected ✓';
        statusSpan.className = 'connected';
      } else {
        statusSpan.textContent = 'Connection failed';
        statusSpan.className = 'disconnected';
      }
    });
  });
});

// Test connection
testButton.addEventListener('click', () => {
  const port = parseInt(portInput.value, 10);
  if (isNaN(port) || port < 1 || port > 65535) {
    statusSpan.textContent = 'Invalid port number';
    statusSpan.className = 'disconnected';
    return;
  }

  statusSpan.textContent = 'Testing connection...';
  statusSpan.className = 'testing';
  testButton.disabled = true;

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);

  fetch(`http://127.0.0.1:${port}/health`, { signal: controller.signal })
    .then((response) => {
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      statusSpan.textContent = 'Server reachable ✓';
      statusSpan.className = 'connected';
    })
    .catch(() => {
      statusSpan.textContent = 'Server not reachable';
      statusSpan.className = 'disconnected';
    })
    .finally(() => {
      clearTimeout(timeout);
      testButton.disabled = false;
    });
});

function updateStatusDisplay() {
  clearTimeout(statusTimer);
  chrome.runtime.sendMessage({ type: 'getConnectionStatus' }, (response) => {
    if (chrome.runtime.lastError || !response) {
      statusSpan.textContent = 'Disconnected';
      statusSpan.className = 'disconnected';
      return;
    }
    renderPower(response.enabled !== false);
    if (response.enabled === false) {
      showStopped();
    } else if (response.connected) {
      statusSpan.textContent = 'Connected ✓';
      statusSpan.className = 'connected';
    } else if (response.connecting) {
      statusSpan.textContent = 'Connecting...';
      statusSpan.className = 'testing';
      statusTimer = setTimeout(updateStatusDisplay, 1000);
    } else {
      statusSpan.textContent = 'Waiting for acrawl';
      statusSpan.className = 'disconnected';
      statusTimer = setTimeout(updateStatusDisplay, 1500);
    }
  });
}
