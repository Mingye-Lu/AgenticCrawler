'use strict';

async function handleSaveFile(tabId, payload) {
  const { url } = payload;
  if (!url) throw new Error('save_file requires payload.url');

  let response;
  try {
    response = await fetch(url);
  } catch (e) {
    throw new Error(`Failed to fetch ${url}: ${e.message}`);
  }

  if (!response.ok) {
    throw new Error(`HTTP ${response.status} fetching ${url}`);
  }

  const buffer = await response.arrayBuffer();
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
