'use strict';

async function handleFill(tabId, payload) {
  const selector = payload?.selector;
  const value = payload?.value;

  if (!selector) {
    throw new Error('fill requires payload.selector');
  }
  if (typeof value !== 'string') {
    throw new Error('fill requires payload.value as a string');
  }

  await ensureAttached(tabId);

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) {
        return { ok: false, error: 'Element not found' };
      }

      el.focus();
      el.scrollIntoView({ block: 'center', inline: 'center' });

      const tag = el.tagName.toLowerCase();
      if (tag === 'select') {
        el.value = ${JSON.stringify(value)};
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        return { ok: true };
      }

      if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLTextAreaElement)) {
        return { ok: false, error: 'Element is not a fillable input' };
      }

      const proto = el instanceof HTMLInputElement
        ? HTMLInputElement.prototype
        : HTMLTextAreaElement.prototype;
      const nativeSetter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;

      if (nativeSetter) {
        nativeSetter.call(el, ${JSON.stringify(value)});
      } else {
        el.value = ${JSON.stringify(value)};
      }

      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return { ok: true };
    })()`,
    returnByValue: true,
  });

  const result = res.result?.value;
  if (!result?.ok) {
    throw new Error(result?.error ?? 'fill failed');
  }

  return { filled: true };
}
