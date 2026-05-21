'use strict';

async function handleNavigate(tabId, payload) {
  const url = payload?.url;
  if (!url) {
    throw new Error('navigate requires payload.url');
  }

  await ensureAttached(tabId);
  await enablePageEvents(tabId);

  const loadPromise = waitForLoad(tabId, 30000);
  const navResult = await cdp(tabId, 'Page.navigate', { url });

  if (navResult.errorText) {
    throw new Error(`Navigation failed: ${navResult.errorText}`);
  }

  await loadPromise.catch(() => {});

  return getPageContent(tabId);
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
