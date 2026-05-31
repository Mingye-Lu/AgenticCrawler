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
      function resolveElement(raw) {
        if (/[#.\\[\\]:>~+\\s]/.test(raw)) return document.querySelector(raw);
        const lower = raw.toLowerCase();
        const candidates = [
          '#' + raw,
          '#' + lower,
          '[name="' + raw + '"]',
          '[name="' + lower + '"]',
          'input[name="' + raw + '"]',
          'input[name="' + lower + '"]',
          'textarea[name="' + raw + '"]',
          'textarea[name="' + lower + '"]',
          'select[name="' + raw + '"]',
          'select[name="' + lower + '"]',
          '[placeholder="' + raw + '"]',
          'input[aria-label="' + raw + '"]',
          'textarea[aria-label="' + raw + '"]',
        ];
        for (const sel of candidates) {
          try { const el = document.querySelector(sel); if (el) return el; } catch (_) {}
        }
        const labels = document.querySelectorAll('label');
        for (const lbl of labels) {
          if (lbl.textContent.trim().toLowerCase() === lower) {
            const forAttr = lbl.getAttribute('for');
            if (forAttr) { const t = document.getElementById(forAttr); if (t) return t; }
            const input = lbl.querySelector('input, textarea, select');
            if (input) return input;
          }
        }
        return document.querySelector(raw);
      }
      const el = resolveElement(${JSON.stringify(selector)});
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
