'use strict';

const SAVE_FILE_MAX_BYTES = 50 * 1024 * 1024; // 50 MB

// Headers that fetch() cannot set; must be injected via declarativeNetRequest
const RESTRICTED_FETCH_HEADERS = new Set([
  'referer', 'origin', 'user-agent', 'host', 'cookie', 'cookie2',
]);

async function handleSaveFile(tabId, payload) {
  const { url, headers = {} } = payload;
  if (!url) throw new Error('save_file requires payload.url');

  // Split: headers fetch can set vs browser-controlled headers it cannot
  const fetchHeaders = {};
  const restrictedHeaders = {};
  for (const [key, value] of Object.entries(headers)) {
    if (RESTRICTED_FETCH_HEADERS.has(key.toLowerCase())) {
      restrictedHeaders[key] = value;
    } else {
      fetchHeaders[key] = value;
    }
  }

  // Inject browser-restricted headers at the network layer via declarativeNetRequest
  let ruleId = null;
  if (Object.keys(restrictedHeaders).length > 0) {
    ruleId = (Date.now() % 2147483647) + 1;
    await chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [{
        id: ruleId,
        priority: 1,
        action: {
          type: 'modifyHeaders',
          requestHeaders: Object.entries(restrictedHeaders).map(([header, value]) => ({
            header,
            operation: 'set',
            value,
          })),
        },
        condition: {
          urlFilter: url,
          resourceTypes: ['xmlhttprequest', 'other'],
        },
      }],
      removeRuleIds: [],
    });
  }

  try {
    let response;
    try {
      response = await fetch(url, { credentials: 'include', headers: fetchHeaders });
    } catch (e) {
      throw new Error(`Failed to fetch ${url}: ${e.message}`);
    }

    if (!response.ok) {
      throw new Error(`HTTP ${response.status} fetching ${url}`);
    }

    const contentLength = response.headers.get('content-length');
    if (contentLength && parseInt(contentLength, 10) > SAVE_FILE_MAX_BYTES) {
      throw new Error(`File too large (${contentLength} bytes, max ${SAVE_FILE_MAX_BYTES})`);
    }

    const buffer = await response.arrayBuffer();
    if (buffer.byteLength > SAVE_FILE_MAX_BYTES) {
      throw new Error(`File too large (${buffer.byteLength} bytes, max ${SAVE_FILE_MAX_BYTES})`);
    }

    const bytes = new Uint8Array(buffer);
    let binary = '';
    const CHUNK = 8192;
    for (let i = 0; i < bytes.length; i += CHUNK) {
      binary += String.fromCharCode(...bytes.slice(i, i + CHUNK));
    }
    const data_base64 = btoa(binary);

    return {
      data_base64,
      size_bytes: bytes.length,
    };
  } finally {
    if (ruleId !== null) {
      chrome.declarativeNetRequest.updateDynamicRules({
        addRules: [],
        removeRuleIds: [ruleId],
      }).catch(() => {}); // best-effort cleanup
    }
  }
}
