'use strict';

async function handleSelectOption(tabId, payload) {
  const selector = payload?.selector;
  const value = payload?.value;
  if (!selector) {
    throw new Error('select_option requires payload.selector');
  }
  if (value === undefined || value === null) {
    throw new Error('select_option requires payload.value');
  }

  await ensureAttached(tabId);

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el || el.tagName.toLowerCase() !== 'select') {
        return { ok: false, error: 'Element not found or not a select' };
      }
      el.value = ${JSON.stringify(value)};
      el.dispatchEvent(new Event('change', { bubbles: true }));
      el.dispatchEvent(new Event('input', { bubbles: true }));
      return { ok: true, selected: el.value };
    })()`,
    returnByValue: true,
  });

  const result = res.result?.value;
  if (!result?.ok) throw new Error(result?.error ?? 'select_option failed');

  return { selected: result.selected };
}
