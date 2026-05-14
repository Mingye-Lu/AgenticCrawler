'use strict';

async function handleClick(tabId, payload) {
  const selector = payload?.selector;
  if (!selector) {
    throw new Error('click requires payload.selector');
  }

  await ensureAttached(tabId);

  await cdp(tabId, 'Runtime.evaluate', {
    expression: `document.querySelector(${JSON.stringify(selector)})?.scrollIntoView({block:'center',inline:'center'})`,
  });

  const coordRes = await cdp(tabId, 'Runtime.evaluate', {
    expression: `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) {
        return null;
      }
      const rect = el.getBoundingClientRect();
      return {
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2,
        width: rect.width,
        height: rect.height,
      };
    })()`,
    returnByValue: true,
  });

  const coords = coordRes.result?.value;
  if (!coords) {
    throw new Error(`Element not found: ${selector}`);
  }
  if (coords.width <= 0 || coords.height <= 0) {
    throw new Error(`Element is not visible: ${selector}`);
  }

  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x: coords.x,
    y: coords.y,
    button: 'none',
    buttons: 0,
  });
  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: coords.x,
    y: coords.y,
    button: 'left',
    buttons: 1,
    clickCount: 1,
  });
  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: coords.x,
    y: coords.y,
    button: 'left',
    buttons: 0,
    clickCount: 1,
  });

  return { clicked: true };
}
