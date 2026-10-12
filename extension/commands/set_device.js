'use strict';

async function handleSetDevice(tabId, payload) {
  await ensureAttached(tabId);

  // Mobile presets emulate an exact viewport. Everything else follows the real
  // window: any earlier override is cleared (so manual resizing works again)
  // and, when a size is requested, the browser window itself is resized.
  const mobile = payload.isMobile === true;
  // Chromium can't shrink a window below ~500px of viewport width, so narrower
  // sizes must be emulated even when the device isn't marked mobile.
  const emulate = payload.viewport && (mobile || payload.viewport.width < 500);
  const dpr = payload.deviceScaleFactor ?? 1.0;
  if (emulate) {
    await cdp(tabId, 'Emulation.setDeviceMetricsOverride', {
      width: payload.viewport.width,
      height: payload.viewport.height,
      deviceScaleFactor: dpr,
      mobile,
    });
  } else if (dpr !== 1.0 || mobile) {
    // width/height 0 = keep the real window size, override only DPR/mobile.
    await cdp(tabId, 'Emulation.setDeviceMetricsOverride', {
      width: 0,
      height: 0,
      deviceScaleFactor: dpr,
      mobile,
    });
  } else {
    await cdp(tabId, 'Emulation.clearDeviceMetricsOverride', {});
  }
  if (!emulate && payload.viewport) {
    const dims = await cdp(tabId, 'Runtime.evaluate', {
      expression: '[outerWidth - innerWidth, outerHeight - innerHeight]',
      returnByValue: true,
    });
    const [dx, dy] = dims.result?.value ?? [0, 0];
    const tab = await chrome.tabs.get(tabId);
    await chrome.windows.update(tab.windowId, {
      state: 'normal',
      width: payload.viewport.width + dx,
      height: payload.viewport.height + dy,
    });
  }
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
