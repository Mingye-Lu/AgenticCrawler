'use strict';

function evaluationCandidates(script) {
  return [script, `(async () => (${script}))()`, `(async () => { ${script} })()`];
}

async function selectEvaluationForm(tabId, script) {
  try {
    await cdp(tabId, 'Runtime.enable', {});
  } catch (_) {
    return script;
  }

  for (const candidate of evaluationCandidates(script)) {
    let probe;
    try {
      probe = await cdp(tabId, 'Runtime.compileScript', {
        expression: candidate,
        sourceURL: '',
        persistScript: false,
      });
    } catch (_) {
      return script;
    }
    if (!probe.exceptionDetails) {
      return candidate;
    }
  }

  return script;
}

async function handleExecuteJs(tabId, payload) {
  await ensureAttached(tabId);

  const { script } = payload || {};
  if (!script) throw new Error('execute_js requires payload.script');

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: await selectEvaluationForm(tabId, script),
    returnByValue: true,
    awaitPromise: true,
  });

  if (res.exceptionDetails) {
    throw new Error(cdpExceptionMessage(res.exceptionDetails, 'JS execution threw exception'));
  }

  return { value: res.result?.value };
}
