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

async function ensureAttached(tabId) {
  await new Promise((resolve, reject) => {
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
