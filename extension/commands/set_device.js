'use strict';

async function handleSetDevice(tabId, payload) {
  await ensureAttached(tabId);

  const isDesktopReset = !payload || (!payload.viewport && !payload.userAgent &&
    payload.isMobile === false && payload.hasTouch === false);

  if (isDesktopReset || (payload.isMobile === false && payload.hasTouch === false &&
    payload.deviceScaleFactor === 1)) {
    // Reset to desktop defaults
    await cdp(tabId, 'Emulation.clearDeviceMetricsOverride', {});
    await cdp(tabId, 'Emulation.setTouchEmulationEnabled', { enabled: false });
    await cdp(tabId, 'Network.setUserAgentOverride', { userAgent: '' });
  } else {
    // Apply mobile/custom device emulation
    const metrics = {
      width: payload.viewport?.width ?? 375,
      height: payload.viewport?.height ?? 667,
      deviceScaleFactor: payload.deviceScaleFactor ?? 2.0,
      mobile: payload.isMobile ?? true,
    };
    await cdp(tabId, 'Emulation.setDeviceMetricsOverride', metrics);
    await cdp(tabId, 'Emulation.setTouchEmulationEnabled', {
      enabled: payload.hasTouch ?? payload.isMobile ?? true,
    });

    if (payload.userAgent) {
      await cdp(tabId, 'Network.setUserAgentOverride', { userAgent: payload.userAgent });
    }
  }

  // Reload page to apply changes
  await enablePageEvents(tabId);
  const loadPromise = waitForLoad(tabId, 15000);
  await cdp(tabId, 'Page.reload', {});
  await loadPromise.catch(() => {});

  // Get result
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
