'use strict';

document.getElementById('version').textContent = 'v' + chrome.runtime.getManifest().version;
const portInput = document.getElementById('port');
// Set by pairing; never typed by the user.
let savedToken = '';
const saveButton = document.getElementById('save');
const testButton = document.getElementById('test');
const statusSpan = document.getElementById('status');

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
  chrome.storage.local.get({ port: 19876, token: '' }, (items) => {
    portInput.value = items.port;
    savedToken = items.token;
    updateStatusDisplay();
    // Poll even with a saved token: a stale one (reset config, another acrawl
    // instance) leaves the extension disconnected and in need of a new pairing.
    pollPairing();
    setInterval(pollPairing, 2000);
    setInterval(tickPairing, 1000);
  });
});

// ----- Pairing: the code is shown on the acrawl side and typed here -----

const pairBox = document.getElementById('pair');
const pairHost = document.getElementById('pairHost');
const pairMeta = document.getElementById('pairMeta');
const pairCode = document.getElementById('pairCode');
const pairButton = document.getElementById('pairButton');
const pairNote = document.getElementById('pairNote');
let pairDeadline = 0;

function setPairNote(text, isError) {
  pairNote.textContent = text;
  pairNote.className = isError ? 'error' : '';
}

function showWaiting() {
  pairDeadline = 0;
  if (savedToken) {
    pairBox.className = '';
    return;
  }
  pairBox.className = 'waiting';
  pairHost.textContent = '';
  pairMeta.textContent = '';
  pairCode.style.display = 'none';
  pairButton.style.display = 'none';
  setPairNote('Not paired. Run /extension in acrawl, or call an acrawl tool from your agent, and a pairing request appears here.', false);
}

function showOffer(offer) {
  const host = offer.host;
  pairBox.className = 'offer';
  pairHost.textContent = `${host.client} wants to pair`;
  pairMeta.textContent = `${host.mode === 'mcp' ? 'acrawl mcp' : 'acrawl REPL'} · pid ${host.pid} · ${host.cwd}`;
  pairCode.style.display = '';
  pairButton.style.display = '';
  pairDeadline = Date.now() + offer.expires_in_secs * 1000;
  tickPairing();
}

function tickPairing() {
  if (!pairDeadline) {
    return;
  }
  const left = Math.max(0, Math.round((pairDeadline - Date.now()) / 1000));
  if (left === 0) {
    showWaiting();
    return;
  }
  const mm = Math.floor(left / 60);
  const ss = String(left % 60).padStart(2, '0');
  if (!pairNote.className) {
    setPairNote(`Enter the code shown in your acrawl session. Expires in ${mm}:${ss}.`, false);
  }
}

function isConnected() {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage({ type: 'getConnectionStatus' }, (response) => {
      resolve(Boolean(!chrome.runtime.lastError && response && response.connected));
    });
  });
}

async function pollPairing() {
  if (await isConnected()) {
    pairDeadline = 0;
    pairBox.className = '';
    return;
  }
  try {
    const res = await fetch(`http://127.0.0.1:${parseInt(portInput.value, 10)}/pair/info`);
    const { offer } = await res.json();
    if (!offer) {
      showWaiting();
    } else if (!pairDeadline) {
      showOffer(offer);
    } else {
      pairDeadline = Date.now() + offer.expires_in_secs * 1000;
    }
  } catch {
    showWaiting();
  }
}

pairButton.addEventListener('click', async () => {
  pairButton.disabled = true;
  try {
    const res = await fetch(`http://127.0.0.1:${parseInt(portInput.value, 10)}/pair`, {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain' },
      body: JSON.stringify({ code: pairCode.value }),
    });
    if (res.ok) {
      const { token } = await res.json();
      savedToken = token;
      chrome.storage.local.set({ port: parseInt(portInput.value, 10), token }, () => {
        pairBox.className = '';
        pairDeadline = 0;
        chrome.runtime.sendMessage({ type: 'reconnect' }, () => updateStatusDisplay());
      });
    } else if (res.status === 403) {
      pairCode.value = '';
      setPairNote('Wrong code. Too many wrong codes cancel the request.', true);
    } else {
      setPairNote('That request expired. Ask acrawl for a new code.', true);
    }
  } catch {
    setPairNote('Could not reach acrawl.', true);
  } finally {
    pairButton.disabled = false;
  }
});

pairCode.addEventListener('input', () => {
  if (pairNote.className) {
    setPairNote('', false);
  }
});
pairCode.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    pairButton.click();
  }
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
