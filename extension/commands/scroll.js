'use strict';

async function handleScroll(tabId, payload) {
  await ensureAttached(tabId);

  const direction = payload?.direction ?? 'down';
  const pixels = payload?.pixels ?? 500;
  const delta = direction === 'up' ? -pixels : pixels;

  await cdp(tabId, 'Runtime.evaluate', {
    expression: `window.scrollBy(0, ${delta})`,
  });

  return { scrolled: true, direction, pixels };
}
