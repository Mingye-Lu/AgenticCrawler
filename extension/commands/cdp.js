'use strict';

function cdp(tabId, method, params = {}) {
  return new Promise((resolve, reject) => {
    chrome.debugger.sendCommand({ tabId }, method, params, (result) => {
      if (chrome.runtime.lastError) {
        reject(new Error(chrome.runtime.lastError.message));
      } else {
        resolve(result || {});
      }
    });
  });
}

function describeThrownValue(value) {
  if (typeof value === 'string') {
    return value;
  }
  try {
    return JSON.stringify(value);
  } catch (_) {
    return String(value);
  }
}

function cdpExceptionMessage(exceptionDetails, fallbackMessage) {
  const details = exceptionDetails || {};
  const thrown = details.exception || {};
  const candidates = [
    thrown.description,
    thrown.value === undefined ? undefined : describeThrownValue(thrown.value),
    thrown.className,
    details.text,
  ];
  const message = candidates.find(
    (candidate) => typeof candidate === 'string' && candidate.trim() !== ''
  );
  return message ? message.trim() : fallbackMessage;
}

function attachDebugger(tabId) {
  return new Promise((resolve, reject) => {
    chrome.debugger.attach({ tabId }, '1.3', () => {
      if (chrome.runtime.lastError) {
        const message = chrome.runtime.lastError.message || '';
        if (!message.includes('already attached')) {
          reject(new Error(message));
          return;
        }
      }
      resolve();
    });
  });
}

// A failed navigation (cert error, DNS failure) commits a chrome-error:// page, which
// detaches the debugger and refuses re-attach. Navigating the tab away is the only way
// back, so reset it to about:blank and attach again.
async function resetToBlank(tabId) {
  await chrome.tabs.update(tabId, { url: 'about:blank' });
  for (let i = 0; i < 50; i++) {
    const tab = await chrome.tabs.get(tabId);
    if (tab.url === 'about:blank' && tab.status === 'complete') return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

async function ensureAttached(tabId) {
  try {
    await attachDebugger(tabId);
  } catch (e) {
    if (!e.message.includes('Cannot attach to this target')) throw e;
    await resetToBlank(tabId);
    await attachDebugger(tabId).catch((retryError) => {
      throw new Error(
        `${retryError.message} The tab could not be recovered; close it or re-run /extension.`
      );
    });
  }
  await setupAutoAttach(tabId);
}

function waitForLoad(tabId, timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      chrome.debugger.onEvent.removeListener(listener);
      reject(new Error('Navigation timeout'));
    }, timeoutMs);

    function listener(source, method, params) {
      if (source.tabId !== tabId) {
        return;
      }
      if (method === 'Page.loadEventFired' ||
          (method === 'Page.lifecycleEvent' && params && params.name === 'load')) {
        clearTimeout(timer);
        chrome.debugger.onEvent.removeListener(listener);
        resolve();
      }
    }

    chrome.debugger.onEvent.addListener(listener);
  });
}

async function enablePageEvents(tabId) {
  await cdp(tabId, 'Page.enable');
}

async function setupAutoAttach(tabId) {
  await cdp(tabId, 'Target.setAutoAttach', {
    autoAttach: true,
    waitForDebuggerOnStart: false,
    flatten: true,
  }).catch(() => {}); // Ignore errors on older Chrome versions
}

// Auto-dismiss JavaScript dialogs (alert, confirm, prompt, beforeunload)
chrome.debugger.onEvent.addListener((source, method, params) => {
  if (method === 'Page.javascriptDialogOpening') {
    chrome.debugger.sendCommand(
      { tabId: source.tabId },
      'Page.handleJavaScriptDialog',
      { accept: true, promptText: '' }
    );
  }
});

async function getPageContent(tabId) {
  const [titleRes, htmlRes] = await Promise.all([
    cdp(tabId, 'Runtime.evaluate', {
      expression: 'document.title',
      returnByValue: true,
    }),
    cdp(tabId, 'Runtime.evaluate', {
      expression: 'document.documentElement.outerHTML',
      returnByValue: true,
    }),
  ]);

  return {
    title: titleRes.result?.value ?? '',
    html: htmlRes.result?.value ?? '',
  };
}
