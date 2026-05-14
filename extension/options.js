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
    statusSpan.textContent = 'Settings saved';
    statusSpan.className = 'connected';
    setTimeout(() => updateStatusDisplay(), 1500);
  });
});

// Test connection
testButton.addEventListener('click', () => {
  const port = parseInt(portInput.value, 10);
  const token = tokenInput.value.trim();

  if (isNaN(port) || port < 1 || port > 65535) {
    statusSpan.textContent = 'Invalid port number';
    statusSpan.className = 'disconnected';
    return;
  }

  if (!token) {
    statusSpan.textContent = 'Token required';
    statusSpan.className = 'disconnected';
    return;
  }

  statusSpan.textContent = 'Testing connection...';
  statusSpan.className = 'testing';
  testButton.disabled = true;

  const ws = new WebSocket(`ws://127.0.0.1:${port}/bridge?token=${encodeURIComponent(token)}`);
  const timeout = setTimeout(() => {
    ws.close();
    statusSpan.textContent = 'Connection timeout';
    statusSpan.className = 'disconnected';
    testButton.disabled = false;
  }, 5000);

  ws.onopen = () => {
    clearTimeout(timeout);
    ws.send(JSON.stringify({ type: 'ping' }));
  };

  ws.onmessage = (event) => {
    clearTimeout(timeout);
    try {
      const msg = JSON.parse(event.data);
      if (msg.type === 'pong') {
        statusSpan.textContent = 'Connected ✓';
        statusSpan.className = 'connected';
      }
    } catch (e) {
      // ignore
    }
    ws.close();
    testButton.disabled = false;
  };

  ws.onerror = () => {
    clearTimeout(timeout);
    statusSpan.textContent = 'Connection failed';
    statusSpan.className = 'disconnected';
    testButton.disabled = false;
  };

  ws.onclose = () => {
    clearTimeout(timeout);
    if (statusSpan.className !== 'connected') {
      statusSpan.textContent = 'Disconnected';
      statusSpan.className = 'disconnected';
    }
    testButton.disabled = false;
  };
});

function updateStatusDisplay() {
  chrome.runtime.sendMessage({ type: 'getConnectionStatus' }, (response) => {
    if (response && response.connected) {
      statusSpan.textContent = 'Connected ✓';
      statusSpan.className = 'connected';
    } else {
      statusSpan.textContent = 'Disconnected';
      statusSpan.className = 'disconnected';
    }
  });
}
