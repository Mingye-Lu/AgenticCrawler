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
}

function waitForLoad(tabId, timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      chrome.debugger.onEvent.removeListener(listener);
      reject(new Error('Navigation timeout'));
    }, timeoutMs);

    function listener(source, method) {
      if (source.tabId !== tabId) {
        return;
      }
      if (method === 'Page.loadEventFired' || method === 'Page.lifecycleEvent') {
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
