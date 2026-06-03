'use strict';

async function handleWait(tabId, payload) {
  await ensureAttached(tabId);

  const { selector, seconds, state } = payload || {};

  if (selector) {
    const timeoutMs = payload.timeout_ms ?? 30000;
    const start = Date.now();

    while (Date.now() - start < timeoutMs) {
      let checkExpr;
      if (state === 'visible') {
        checkExpr = `(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return false; const s = getComputedStyle(el); if (s.display === 'none' || s.visibility === 'hidden') return false; const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; })()`;
      } else if (state === 'hidden') {
        checkExpr = `(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return true; const s = getComputedStyle(el); if (s.display === 'none' || s.visibility === 'hidden') return true; const r = el.getBoundingClientRect(); return r.width === 0 && r.height === 0; })()`;
      } else if (state === 'detached') {
        checkExpr = `!document.querySelector(${JSON.stringify(selector)})`;
      } else {
        // 'attached' or default: element exists in DOM
        checkExpr = `!!document.querySelector(${JSON.stringify(selector)})`;
      }

      const found = await cdp(tabId, 'Runtime.evaluate', {
        expression: checkExpr,
        returnByValue: true,
      });

      if (found.result?.value === true) {
        return { found: true, selector, state: state || 'attached' };
      }

      await new Promise(resolve => setTimeout(resolve, 200));
    }

    return { found: false, selector, state: state || 'attached', timed_out: true };
  }

  if (seconds) {
    await new Promise(resolve => setTimeout(resolve, seconds * 1000));
    return { waited: seconds };
  }

  return { waited: 0 };
}
