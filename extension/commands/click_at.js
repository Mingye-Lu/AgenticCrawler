'use strict';

async function handleClickAt(tabId, payload) {
  const x = payload?.x;
  const y = payload?.y;
  if (typeof x !== 'number' || typeof y !== 'number') {
    throw new Error('click_at requires numeric payload.x and payload.y');
  }

  await ensureAttached(tabId);

  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x,
    y,
    button: 'none',
    buttons: 0,
  });
  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x,
    y,
    button: 'left',
    buttons: 1,
    clickCount: 1,
  });
  await cdp(tabId, 'Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x,
    y,
    button: 'left',
    buttons: 0,
    clickCount: 1,
  });

  return { clicked: true, x, y };
}
