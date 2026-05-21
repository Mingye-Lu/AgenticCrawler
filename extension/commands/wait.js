'use strict';

async function handleWait(tabId, payload) {
  await ensureAttached(tabId);

  const { selector, seconds } = payload || {};

  if (selector) {
    const timeoutMs = payload.timeout_ms ?? 30000;
    const start = Date.now();

    while (Date.now() - start < timeoutMs) {
      const found = await cdp(tabId, 'Runtime.evaluate', {
        expression: `!!document.querySelector(${JSON.stringify(selector)})`,
        returnByValue: true,
      });

      if (found.result?.value === true) {
        return { found: true, selector };
      }

      await new Promise(resolve => setTimeout(resolve, 200));
    }

    return { found: false, selector, timed_out: true };
  }

  if (seconds) {
    await new Promise(resolve => setTimeout(resolve, seconds * 1000));
    return { waited: seconds };
  }

  return { waited: 0 };
}
