'use strict';

async function handleSetDevice(tabId, payload) {
  await ensureAttached(tabId);

  // Always apply the requested emulation settings explicitly.
  // The Rust handler sends fully-resolved options (viewport, UA, DPR, mobile, touch)
  // for both presets and custom configs. We never "guess" defaults here.
  const metrics = {
    width: payload.viewport?.width ?? 1920,
    height: payload.viewport?.height ?? 1080,
    deviceScaleFactor: payload.deviceScaleFactor ?? 1.0,
    mobile: payload.isMobile ?? false,
  };
  await cdp(tabId, 'Emulation.setDeviceMetricsOverride', metrics);
  await cdp(tabId, 'Emulation.setTouchEmulationEnabled', {
    enabled: payload.hasTouch ?? false,
  });

  if (payload.userAgent) {
    await cdp(tabId, 'Network.setUserAgentOverride', { userAgent: payload.userAgent });
  } else {
      await cdp(tabId, 'Network.setUserAgentOverride', { userAgent: '' });
  }

  await enablePageEvents(tabId);
  const loadPromise = waitForLoad(tabId, 15000);
  await cdp(tabId, 'Page.reload', {});
  await loadPromise.catch(() => {});

  const [urlRes, titleRes] = await Promise.all([
    cdp(tabId, 'Runtime.evaluate', { expression: 'location.href', returnByValue: true }),
    cdp(tabId, 'Runtime.evaluate', { expression: 'document.title', returnByValue: true }),
  ]);

  return {
    viewport: payload.viewport || null,
    userAgent: payload.userAgent || null,
    url: urlRes.result?.value ?? '',
    title: titleRes.result?.value ?? '',
  };
}
