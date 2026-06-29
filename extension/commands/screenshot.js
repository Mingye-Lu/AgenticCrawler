'use strict';

async function handleScreenshot(tabId, payload) {
  await ensureAttached(tabId);

  const format = payload.format || 'png';
  const cdpFormat = format === 'webp' ? 'webp' : format === 'jpeg' ? 'jpeg' : 'png';

  const captureOpts = {
    format: cdpFormat,
    captureBeyondViewport: !!payload.fullPage,
  };

  if (payload.quality != null && (cdpFormat === 'jpeg' || cdpFormat === 'webp')) {
    captureOpts.quality = payload.quality;
  }

  if (payload.selector) {
    const { result } = await cdp(tabId, 'Runtime.evaluate', {
      expression: `(() => {
        const el = document.querySelector(${JSON.stringify(payload.selector)});
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.x, y: r.y, width: r.width, height: r.height };
      })()`,
      returnByValue: true,
    });
    if (result.value) {
      captureOpts.clip = {
        x: result.value.x,
        y: result.value.y,
        width: result.value.width,
        height: result.value.height,
        scale: 1,
      };
    }
  }

  const result = await cdp(tabId, 'Page.captureScreenshot', captureOpts);

  const base64 = result.data;
  if (!base64) throw new Error('captureScreenshot returned no data');

  const sizeBytes = Math.round(base64.length * 0.75);

  return { screenshot_base64: base64, size_bytes: sizeBytes };
}
