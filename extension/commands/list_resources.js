'use strict';

function listResourcesScript() {
  const links = Array.from(document.querySelectorAll('a[href]')).map(a => ({ href: a.href, text: a.textContent.trim() }));
  const images = Array.from(document.querySelectorAll('img')).map(img => ({ src: img.src, alt: img.alt }));
  const forms = Array.from(document.querySelectorAll('form')).map(f => ({ action: f.action, method: f.method, id: f.id }));
  return { links, images, forms };
}

async function handleListResources(tabId) {
  await ensureAttached(tabId);

  const res = await cdp(tabId, 'Runtime.evaluate', {
    expression: '(' + listResourcesScript.toString() + ')()',
    returnByValue: true,
  });

  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.text || 'list_resources script threw exception');
  }

  return res.result?.value || { links: [], images: [], forms: [] };
}
