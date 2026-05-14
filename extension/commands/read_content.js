'use strict';

function readContentScript(payload) {
  const { heading, selector, offset, max_chars } = payload;
  let rawContent = '';
  let matches_count = 0;
  let found = false;
  if (heading) {
    const allHeadings = Array.from(document.querySelectorAll('h1,h2,h3,h4,h5,h6'));
    const matches = allHeadings.filter(el => el.innerText.trim().toLowerCase() === heading.toLowerCase());
    matches_count = matches.length;
    if (matches.length > 0) {
      found = true;
      const el = matches[0];
      const level = parseInt(el.tagName[1]);
      let sibling = el.nextElementSibling;
      while (sibling) {
        const sibTag = sibling.tagName.toLowerCase();
        if (/^h[1-6]$/.test(sibTag) && parseInt(sibTag[1]) <= level) break;
        rawContent += (sibling.innerText || '') + '\n';
        sibling = sibling.nextElementSibling;
      }
    } else {
      const hint = allHeadings.slice(0, 20).map(el => el.innerText.trim());
      return { content: '', found: false, total_chars: 0, offset: 0, has_more: false, truncated: false, matches_count: 0, hint };
    }
  } else if (selector) {
    const els = Array.from(document.querySelectorAll(selector));
    matches_count = els.length;
    found = els.length > 0;
    rawContent = els.map(el => el.innerText || '').join('\n');
  }
  const total_chars = rawContent.length;
  const sliced = rawContent.slice(offset, offset + max_chars);
  const has_more = offset + max_chars < total_chars;
  const truncated = sliced.length < (total_chars - offset);
  return { content: sliced, found, total_chars, offset, has_more, truncated, matches_count };
}

async function handleReadContent(tabId, payload) {
  await ensureAttached(tabId);

  const evalPayload = {
    heading: payload?.heading || null,
    selector: payload?.selector || null,
    offset: payload?.offset || 0,
    max_chars: payload?.max_chars || 10000,
  };

  const expression = `(${readContentScript.toString()})(${JSON.stringify(evalPayload)})`;
  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'read_content script threw exception');
  }

  return res.result?.value || {};
}
