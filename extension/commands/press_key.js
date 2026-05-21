'use strict';

const KEY_MAP = {
  Enter: { key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 },
  Tab: { key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 },
  Escape: { key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 },
  ArrowUp: { key: 'ArrowUp', code: 'ArrowUp', windowsVirtualKeyCode: 38 },
  ArrowDown: { key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 },
  ArrowLeft: { key: 'ArrowLeft', code: 'ArrowLeft', windowsVirtualKeyCode: 37 },
  ArrowRight: { key: 'ArrowRight', code: 'ArrowRight', windowsVirtualKeyCode: 39 },
  Backspace: { key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 },
  Delete: { key: 'Delete', code: 'Delete', windowsVirtualKeyCode: 46 },
  Home: { key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 },
  End: { key: 'End', code: 'End', windowsVirtualKeyCode: 35 },
  PageUp: { key: 'PageUp', code: 'PageUp', windowsVirtualKeyCode: 33 },
  PageDown: { key: 'PageDown', code: 'PageDown', windowsVirtualKeyCode: 34 },
  ' ': { key: ' ', code: 'Space', windowsVirtualKeyCode: 32 },
  Space: { key: ' ', code: 'Space', windowsVirtualKeyCode: 32 },
};

function resolveKey(keyName) {
  if (KEY_MAP[keyName]) return KEY_MAP[keyName];
  const charCode = keyName.charCodeAt(0);
  return {
    key: keyName,
    code: keyName.length === 1 ? `Key${keyName.toUpperCase()}` : keyName,
    windowsVirtualKeyCode: charCode,
    text: keyName.length === 1 ? keyName : undefined,
  };
}

async function handlePressKey(tabId, payload) {
  const key = payload?.key;
  if (!key) {
    throw new Error('press_key requires payload.key');
  }

  await ensureAttached(tabId);

  if (payload.selector) {
    await cdp(tabId, 'Runtime.evaluate', {
      expression: `document.querySelector(${JSON.stringify(payload.selector)})?.focus()`,
    });
  }

  const keyInfo = resolveKey(key);

  await cdp(tabId, 'Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: keyInfo.key,
    code: keyInfo.code,
    windowsVirtualKeyCode: keyInfo.windowsVirtualKeyCode,
    ...(keyInfo.text ? { text: keyInfo.text } : {}),
  });

  await cdp(tabId, 'Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: keyInfo.key,
    code: keyInfo.code,
    windowsVirtualKeyCode: keyInfo.windowsVirtualKeyCode,
  });

  return { pressed: true };
}
