'use strict';

async function handleExecuteJs(tabId, payload) {
  await ensureAttached(tabId);

  const { script } = payload || {};
  if (!script) throw new Error('execute_js requires payload.script');

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: script,
    returnByValue: true,
    awaitPromise: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'JS execution threw exception');
  }

  return { value: res.result?.value };
}
