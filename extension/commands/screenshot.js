'use strict';

async function handleScreenshot(tabId) {
  await ensureAttached(tabId);

  const result = await cdp(tabId, 'Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: true,
  });

  const base64 = result.data;
  if (!base64) throw new Error('captureScreenshot returned no data');

  const sizeBytes = Math.round(base64.length * 0.75);

  return { screenshot_base64: base64, size_bytes: sizeBytes };
}
