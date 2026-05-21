'use strict';

async function handleExportCookies(tabId) {
  await ensureAttached(tabId);
  await ensureNetworkEnabled(tabId);

  // Get current URL
  const urlRes = await cdp(tabId, 'Runtime.evaluate', {
    expression: 'location.href',
    returnByValue: true,
  });
  const url = urlRes.result?.value || '';

  // Get all cookies for the current page
  const cookiesRes = await cdp(tabId, 'Network.getCookies', { urls: [url] });
  const cookies = cookiesRes.cookies || [];

  // Get localStorage
  const storageRes = await cdp(tabId, 'Runtime.evaluate', {
    expression: '(() => { const obj = {}; for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); obj[k] = localStorage.getItem(k); } return obj; })()',
    returnByValue: true,
  });
  const local_storage = storageRes.result?.value || {};

  return { cookies, local_storage, url };
}

async function handleImportCookies(tabId, payload) {
  await ensureAttached(tabId);
  await ensureNetworkEnabled(tabId);
  const state = payload?.state || {};

  // Import cookies
  const cookies = state.cookies || [];
  for (const cookie of cookies) {
    await cdp(tabId, 'Network.setCookie', {
      name: cookie.name,
      value: cookie.value,
      domain: cookie.domain,
      path: cookie.path || '/',
      secure: cookie.secure || false,
      httpOnly: cookie.httpOnly || false,
    }).catch(() => {}); // Ignore individual cookie errors
  }

  // Import localStorage if state.url matches current URL
  if (state.local_storage && Object.keys(state.local_storage).length > 0) {
    const entries = JSON.stringify(state.local_storage);
    await cdp(tabId, 'Runtime.evaluate', {
      expression: `(() => { const entries = ${entries}; for (const [k, v] of Object.entries(entries)) { try { localStorage.setItem(k, v); } catch(_) {} } })()`,
    }).catch(() => {});
  }

  return { imported: true };
}

async function handleImportCookiesOnly(tabId, payload) {
  await ensureAttached(tabId);
  await ensureNetworkEnabled(tabId);
  const state = payload?.state || {};
  const cookies = state.cookies || [];

  for (const cookie of cookies) {
    await cdp(tabId, 'Network.setCookie', {
      name: cookie.name,
      value: cookie.value,
      domain: cookie.domain,
      path: cookie.path || '/',
      secure: cookie.secure || false,
      httpOnly: cookie.httpOnly || false,
    }).catch(() => {});
  }

  return { imported: true };
}

async function handleImportLocalStorage(tabId, payload) {
  await ensureAttached(tabId);
  const state = payload?.state || {};

  if (state.local_storage) {
    const entries = JSON.stringify(state.local_storage);
    await cdp(tabId, 'Runtime.evaluate', {
      expression: `(() => { const entries = ${entries}; for (const [k, v] of Object.entries(entries)) { try { localStorage.setItem(k, v); } catch(_) {} } })()`,
    }).catch(() => {});
  }

  return { imported: true };
}

async function ensureNetworkEnabled(tabId) {
  await cdp(tabId, 'Network.enable').catch(() => {});
}
