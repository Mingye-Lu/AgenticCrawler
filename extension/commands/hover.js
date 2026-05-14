'use strict';

async function handleHover(tabId, payload) {
  const selector = payload?.selector;
  if (!selector) {
    throw new Error('hover requires payload.selector');
  }

  await ensureAttached(tabId);

  await cdp(tabId, 'Runtime.evaluate', {
    expression: `document.querySelector(${JSON.stringify(selector)})?.scrollIntoView({block:'center',inline:'center'})`,
  });

  const coordRes = await cdp(tabId, 'Runtime.evaluate', {
    expression: `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`,
    returnByValue: true,
  });

  const coords = coordRes.result?.value;
  if (!coords) {
    throw new Error(`Element not found: ${selector}`);
  }

  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x: coords.x,
    y: coords.y,
    button: 'none',
    buttons: 0,
  });

  return { hovered: true };
}
