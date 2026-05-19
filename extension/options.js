'use strict';

const portInput = document.getElementById('port');
const tokenInput = document.getElementById('token');
const saveButton = document.getElementById('save');
const testButton = document.getElementById('test');
const statusSpan = document.getElementById('status');

// Load settings on page load
document.addEventListener('DOMContentLoaded', () => {
  chrome.storage.local.get({ port: 19876, token: '' }, (items) => {
    portInput.value = items.port;
    tokenInput.value = items.token;
    updateStatusDisplay();
  });
});

// Save settings
saveButton.addEventListener('click', () => {
  const port = parseInt(portInput.value, 10);
  const token = tokenInput.value.trim();

  if (isNaN(port) || port < 1 || port > 65535) {
    statusSpan.textContent = 'Invalid port number';
    statusSpan.className = 'disconnected';
    return;
  }

  chrome.storage.local.set({ port, token }, () => {
    statusSpan.textContent = 'Connecting...';
    statusSpan.className = 'testing';
    saveButton.disabled = true;
    testButton.disabled = true;
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
  chrome.runtime.sendMessage({ type: 'getConnectionStatus' }, (response) => {
    if (chrome.runtime.lastError || !response) {
      statusSpan.textContent = 'Disconnected';
      statusSpan.className = 'disconnected';
    } else if (response.connected) {
      statusSpan.textContent = 'Connected ✓';
      statusSpan.className = 'connected';
    } else if (response.connecting) {
      statusSpan.textContent = 'Connecting...';
      statusSpan.className = 'testing';
      setTimeout(updateStatusDisplay, 1000);
    } else if (!response.configured) {
      statusSpan.textContent = 'Not configured';
      statusSpan.className = 'disconnected';
    } else {
      statusSpan.textContent = 'Disconnected';
      statusSpan.className = 'disconnected';
      setTimeout(updateStatusDisplay, 1500);
    }
  });
}
