'use strict';

async function handleNavigate(tabId, payload) {
  const url = payload?.url;
  if (!url) {
    throw new Error('navigate requires payload.url');
  }

  await ensureAttached(tabId);
  await enablePageEvents(tabId);
  await cdp(tabId, 'Network.enable', {}).catch(() => {});

  const network = trackNetwork(tabId);
  try {
    const loadPromise = waitForLoad(tabId, 30000);
    const navResult = await cdp(tabId, 'Page.navigate', { url });

    if (navResult.errorText) {
      throw new Error(`Navigation failed: ${navResult.errorText}`);
    }

    await loadPromise.catch(() => {});
    await network.waitForIdle(5000);
  } finally {
    network.stop();
  }
  await waitForDomSettle(tabId).catch(() => {});

  return getPageContent(tabId);
}

// Playwright's networkidle: no request in flight for 500ms. Long-lived requests
// (SSE, long polling) never finish, so the caller caps the wait.
function trackNetwork(tabId) {
  const inflight = new Set();
  let lastChange = Date.now();

  function listener(source, method, params) {
    if (source.tabId !== tabId) {
      return;
    }
    if (method === 'Network.requestWillBeSent') {
      inflight.add(params.requestId);
      lastChange = Date.now();
    } else if (method === 'Network.loadingFinished' || method === 'Network.loadingFailed') {
      inflight.delete(params.requestId);
      lastChange = Date.now();
    }
  }

  chrome.debugger.onEvent.addListener(listener);
  return {
    async waitForIdle(timeoutMs) {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline && !(inflight.size === 0 && Date.now() - lastChange >= 500)) {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    },
    stop() {
      chrome.debugger.onEvent.removeListener(listener);
    },
  };
}

// Client-rendered pages fill the DOM after `load`. Mirror the CloakBrowser bridge
// (networkidle above + visible-text poll): wait until the DOM has been quiet for 500ms and
// shows at least 200 chars of text, giving up after 3s on a quiet near-empty page
// and after 5s on one that keeps mutating.
const DOM_SETTLE_EXPRESSION = `new Promise((resolve) => {
  const start = Date.now();
  let last = start;
  const observer = new MutationObserver(() => { last = Date.now(); });
  observer.observe(document, { subtree: true, childList: true, characterData: true });
  const timer = setInterval(() => {
    const now = Date.now();
    const quiet = now - last >= 500;
    const hasText = (document.body?.innerText?.trim().length ?? 0) >= 200;
    if ((quiet && (hasText || now - start >= 3000)) || now - start >= 5000) {
      clearInterval(timer);
      observer.disconnect();
      resolve(true);
    }
  }, 100);
})`;

function waitForDomSettle(tabId) {
  return cdp(tabId, 'Runtime.evaluate', {
    expression: DOM_SETTLE_EXPRESSION,
    awaitPromise: true,
    returnByValue: true,
  });
}

async function handleGoBack(tabId) {
  await ensureAttached(tabId);
  await enablePageEvents(tabId);

  const loadPromise = waitForLoad(tabId, 15000);

  try {
    await cdp(tabId, 'Page.goBack', {});
  } catch (_) {
    await cdp(tabId, 'Runtime.evaluate', { expression: 'history.back()' });
  }

  await loadPromise.catch(() => {});

  const urlRes = await cdp(tabId, 'Runtime.evaluate', {
    expression: 'location.href',
    returnByValue: true,
  });

  return { url: urlRes.result?.value ?? '' };
}
