'use strict';

const SAVE_FILE_MAX_BYTES = 50 * 1024 * 1024; // 50 MB

async function handleSaveFile(tabId, payload) {
  const { url, headers = {} } = payload;
  if (!url) throw new Error('save_file requires payload.url');

  let response;
  try {
    response = await fetch(url, { credentials: 'include', headers });
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
}
