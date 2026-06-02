#[allow(clippy::needless_raw_string_hashes)]
pub(crate) const PLAYWRIGHT_BRIDGE_NODE_SCRIPT: &str = r#"
const readline = require('node:readline');

function parseHeadless() {
  const raw = process.env.HEADLESS;
  if (raw === undefined) return true;
  const v = String(raw).trim().toLowerCase();
  return !(v === 'false' || v === '0' || v === 'no' || v === 'off');
}

async function resolveFillSelector(pg, raw) {
  if (/^[#.\[]/.test(raw) || /[\[\]:>~+]/.test(raw)) return raw;
  const lower = raw.toLowerCase();
  const candidates = [
    `#${raw}`,
    `#${lower}`,
    `[name="${raw}"]`,
    `[name="${lower}"]`,
    `input[name="${raw}"]`,
    `input[name="${lower}"]`,
    `textarea[name="${raw}"]`,
    `textarea[name="${lower}"]`,
    `select[name="${raw}"]`,
    `select[name="${lower}"]`,
    `[placeholder="${raw}"]`,
    `input[aria-label="${raw}"]`,
    `textarea[aria-label="${raw}"]`,
  ];
  for (const sel of candidates) {
    try {
      const el = await pg.$(sel);
      if (el) return sel;
    } catch (_) {}
  }
  try {
    const resolved = await pg.evaluate((labelText) => {
      const labels = document.querySelectorAll('label');
      for (const lbl of labels) {
        const text = lbl.textContent.trim();
        if (text.toLowerCase() === labelText.toLowerCase()) {
          const forAttr = lbl.getAttribute('for');
          if (forAttr) {
            const target = document.getElementById(forAttr);
            if (target) return `#${forAttr}`;
          }
          const input = lbl.querySelector('input, textarea, select');
          if (input) {
            if (input.id) return `#${input.id}`;
            if (input.name) return `[name="${input.name}"]`;
            const type = input.getAttribute('type') || 'text';
            return `label:has-text("${text}") ${input.tagName.toLowerCase()}[type="${type}"]`;
          }
        }
      }
      return null;
    }, raw);
    if (resolved) {
      try {
        const el = await pg.$(resolved);
        if (el) return resolved;
      } catch (_) {}
    }
  } catch (_) {}
  return raw;
}

async function bootstrap() {
  let launch;
  try {
    ({ launch } = await import('cloakbrowser'));
  } catch (_firstError) {
    // ESM import() does not respect NODE_PATH — manually resolve from it.
    const path = require('node:path');
    const fs = require('node:fs');
    const url = require('node:url');
    let resolved = false;
    for (const dir of (process.env.NODE_PATH || '').split(path.delimiter)) {
      if (!dir) continue;
      const pkgJson = path.join(dir, 'cloakbrowser', 'package.json');
      if (!fs.existsSync(pkgJson)) continue;
      try {
        const pkg = JSON.parse(fs.readFileSync(pkgJson, 'utf8'));
        const entry = pkg.exports?.['.']?.import || pkg.module || pkg.main || 'index.js';
        ({ launch } = await import(url.pathToFileURL(path.join(dir, 'cloakbrowser', entry)).href));
        resolved = true;
        break;
      } catch (_) {}
    }
    if (!resolved) {
      process.stdout.write(JSON.stringify({
        event: 'bridge_bootstrap',
        ok: false,
        error: {
          kind: 'playwright_not_installed',
          message: 'CloakBrowser package not found. Install with `npm install cloakbrowser`.'
        }
      }) + '\n');
      process.exit(1);
      return;
    }
  }
  console.log = (...args) => process.stderr.write(args.map(String).join(' ') + '\n');
  const browser = await launch({ headless: parseHeadless(), humanize: true });
  const context = await browser.newContext();
  let page = await context.newPage();
  const pages = [page];
  context.on('page', (p) => {
    if (!pages.includes(p)) {
      pages.push(p);
    }
  });
  process.stdout.write(JSON.stringify({ event: 'bridge_bootstrap', ok: true }) + '\n');

  async function bypassTurnstileIfPresent(pg) {
    let html = await pg.content();
    if (!html.includes('Checking your browser') && !html.includes('challenge-platform')) {
      return html;
    }
    await pg.mouse.move(120 + Math.random() * 200, 180 + Math.random() * 150);
    await new Promise(r => setTimeout(r, 500 + Math.random() * 800));
    await pg.mouse.move(350 + Math.random() * 250, 280 + Math.random() * 180);
    await new Promise(r => setTimeout(r, 400 + Math.random() * 600));
    for (let i = 0; i < 16; i++) {
      await new Promise(r => setTimeout(r, 500));
      html = await pg.content();
      if (!html.includes('Checking your browser')) break;
    }
    return html;
  }

  const wire = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of wire) {
    let command;
    try {
      command = JSON.parse(line);
    } catch (error) {
      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: false,
        error: { kind: 'invalid_json', message: String(error) }
      }) + '\n');
      continue;
    }

    if (command.action === 'navigate') {
      try {
        await page.goto(command.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        // Wait for SPA API calls to complete. Cap at 3s so pages with
        // persistent connections (WebSocket, SSE, polling) don't hang.
        try {
          await page.waitForLoadState('networkidle', { timeout: 3000 });
        } catch (_) { /* networkidle timed out — proceed with current state */ }
        // `bypassTurnstileIfPresent` already calls `page.content()` after
        // any nudge it performs, so reuse that html instead of fetching
        // again — the earlier `html = await page.content()` overwrite was
        // dead and just doubled the wire-format round-trip.
        const html = await bypassTurnstileIfPresent(page);
        const title = await page.title();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { title, html }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'navigate_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'close') {
      await page.close().catch(() => {});
      await browser.close().catch(() => {});
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { closed: true } }) + '\n');
      process.exit(0);
    }

    if (command.action === 'new_page') {
      try {
        const newPage = await context.newPage();
        if (!pages.includes(newPage)) {
          pages.push(newPage);
        }
        const pageIndex = pages.indexOf(newPage);
        page = newPage;
        let currentUrl = newPage.url();
        if (command.url) {
          await newPage.goto(command.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
          currentUrl = newPage.url();
          await bypassTurnstileIfPresent(newPage);
        }
        await newPage.bringToFront();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { pageIndex, url: currentUrl }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'new_page_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'close_page') {
      try {
        const pageIndex = command.pageIndex;
        if (!Number.isInteger(pageIndex) || pageIndex < 0 || pageIndex >= pages.length || !pages[pageIndex]) {
          process.stdout.write(JSON.stringify({
            event: 'bridge_response',
            ok: false,
            error: { kind: 'close_page_failed', message: `Invalid page index ${pageIndex}` }
          }) + '\n');
          continue;
        }
        const targetPage = pages[pageIndex];
        await targetPage.close();
        pages[pageIndex] = null;
        if (page === targetPage) {
          const fallbackPage = pages.find((entry) => entry);
          if (fallbackPage) {
            page = fallbackPage;
            await page.bringToFront().catch(() => {});
          }
        }
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { closed: true }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'close_page_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'click') {
      try {
        const urlBefore = page.url();
        await page.click(command.selector, { timeout: 5000 });
        const deadline = Date.now() + 2000;
        while (Date.now() < deadline) {
          if (page.url() !== urlBefore) {
            await page.waitForLoadState('domcontentloaded').catch(() => {});
            break;
          }
          await new Promise(r => setTimeout(r, 50));
        }
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true } }) + '\n');
      } catch (mainError) {
        let clickedInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.click(command.selector, { timeout: 2000 });
            clickedInFrame = true;
            break;
          } catch (_) {}
        }
        if (clickedInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'click_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'fill') {
      try {
        const sel = await resolveFillSelector(page, command.selector);
        await page.fill(sel, command.value, { timeout: 5000 });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, resolvedSelector: sel } }) + '\n');
      } catch (mainError) {
        let filledInFrame = false;
        for (const frame of page.frames()) {
          if (frame === page.mainFrame()) continue;
          try {
            await frame.fill(command.selector, command.value, { timeout: 2000 });
            filledInFrame = true;
            break;
          } catch (_) {}
        }
        if (filledInFrame) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true, frame: true } }) + '\n');
        } else {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'fill_failed', message: String(mainError) } }) + '\n');
        }
      }
      continue;
    }

    if (command.action === 'screenshot') {
      try {
        const buffer = await page.screenshot({ type: 'png' });
        const base64Data = buffer.toString('base64');
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { screenshot_base64: base64Data, size_bytes: buffer.length } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'screenshot_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'go_back') {
      try {
        await page.goBack({ waitUntil: 'domcontentloaded', timeout: 30000 });
        const url = page.url();
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'go_back_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'scroll') {
      try {
        const dir = command.direction === 'up' ? -1 : 1;
        const px = (command.pixels || 500) * dir;
        await page.evaluate((y) => window.scrollBy(0, y), px);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { scrolled: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'scroll_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'page_map') {
      try {
        const result = await page.evaluate(() => {
          function cssPath(el) {
            if (el.id) return '#' + CSS.escape(el.id);
            const parts = [];
            let cur = el;
            while (cur && cur !== document.body && cur !== document.documentElement) {
              let seg = cur.tagName.toLowerCase();
              const parent = cur.parentElement;
              if (parent) {
                const sibs = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
                if (sibs.length > 1) seg += ':nth-of-type(' + (sibs.indexOf(cur) + 1) + ')';
              }
              parts.unshift(seg);
              cur = cur.parentElement;
            }
            return parts.join(' > ');
          }

          const headings = Array.from(document.querySelectorAll('h1, h2, h3, h4, h5, h6')).map((el) => {
            const level = parseInt(el.tagName[1]);
            const text = el.innerText.trim();
            const id = el.id || null;
            const selector = cssPath(el);
            let content = '';
            let sibling = el.nextElementSibling;
            while (sibling) {
              const sibTag = sibling.tagName.toLowerCase();
              if (/^h[1-6]$/.test(sibTag) && parseInt(sibTag[1]) <= level) break;
              content += sibling.innerText || '';
              sibling = sibling.nextElementSibling;
            }
            const char_count = content.length;
            const preview = char_count > 100 ? content.slice(0, 100).trim() + '...' : content.trim();
            return { level, text, id, selector, char_count, preview };
          });

          const landmarks = Array.from(document.querySelectorAll('nav, main, aside, article, footer, header, section[aria-label], [role="navigation"], [role="main"], [role="complementary"]')).map((el) => ({
            tag: el.tagName.toLowerCase(),
            role: el.getAttribute('role'),
            id: el.id || null,
            selector: cssPath(el),
            text_preview: (el.innerText || '').trim().slice(0, 120),
          }));
          const total_landmarks = landmarks.length;
          const cappedLandmarks = landmarks.slice(0, 20);

          const MAX_FORMS = 10;
          const MAX_FIELDS_PER_FORM = 30;
          const allForms = Array.from(document.querySelectorAll('form'));
          const total_forms = allForms.length;
          const forms = allForms.slice(0, MAX_FORMS).map((f) => {
            const allFields = Array.from(f.querySelectorAll('input, select, textarea'));
            const fields = allFields.slice(0, MAX_FIELDS_PER_FORM).map((el) => {
              const label = el.id
                ? document.querySelector(`label[for="${CSS.escape(el.id)}"]`)?.textContent?.trim() || el.placeholder || ''
                : el.placeholder || '';
              return {
                name: el.name || '',
                id: el.id || '',
                type: el.type || el.tagName.toLowerCase(),
                label,
                required: Boolean(el.required),
              };
            });
            return {
              action: f.action || '',
              method: f.method || 'get',
              id: f.id || null,
              selector: cssPath(f),
              fields,
              total_fields: allFields.length,
            };
          });

          const seenHrefs = new Set();
          const MAX_LINKS = 50;
          let total_links = 0;
          const links = [];
          for (const a of document.querySelectorAll('a[href]')) {
            const text = (a.textContent || '').trim();
            const rawHref = a.getAttribute('href') || '';
            const href = a.href || rawHref;
            if (!text || rawHref.startsWith('#') || seenHrefs.has(href)) continue;
            seenHrefs.add(href);
            total_links++;
            if (links.length < MAX_LINKS) {
              links.push({ text, href, selector: cssPath(a) });
            }
          }

          const interactive = {
            buttons: document.querySelectorAll('button').length,
            inputs: document.querySelectorAll('input').length,
            selects: document.querySelectorAll('select').length,
            textareas: document.querySelectorAll('textarea').length,
          };

          const meta = {
            title: document.title,
            description: document.querySelector('meta[name="description"]')?.content || '',
            url: window.location.href,
          };

          return { headings, landmarks: cappedLandmarks, forms, links, interactive, meta, total_landmarks, total_forms, total_links };
        });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'page_map_error', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'read_content') {
      try {
        const heading = command.heading || null;
        const selector = command.selector || null;
        const offset = command.offset || 0;
        const max_chars = command.max_chars || 10000;
        const result = await page.evaluate(({ heading, selector, offset, max_chars }) => {
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
        }, { heading, selector, offset, max_chars });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'read_content_error', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'wait_for_selector') {
      try {
        const timeout = command.timeout_ms || 5000;
        await page.waitForSelector(command.selector, { timeout });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: false } }) + '\n');
      }
      continue;
    }

    if (command.action === 'select_option') {
      try {
        await page.selectOption(command.selector, command.value);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'select_option_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'evaluate') {
      try {
        const result = await page.evaluate(command.script);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { value: result } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'evaluate_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'hover') {
      try {
        await page.hover(command.selector);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'hover_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'press_key') {
      try {
        if (command.selector) {
          await page.focus(command.selector);
        }
        await page.keyboard.press(command.key);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'press_key_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'list_resources') {
      try {
        const resources = await page.evaluate(() => {
          const links = Array.from(document.querySelectorAll('a[href]')).map(a => ({ href: a.href, text: a.textContent.trim() }));
          const images = Array.from(document.querySelectorAll('img')).map(img => ({ src: img.src, alt: img.alt }));
          const forms = Array.from(document.querySelectorAll('form')).map(f => ({ action: f.action, method: f.method, id: f.id }));
          return { links, images, forms };
        });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: resources }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'list_resources_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'save_file') {
      try {
        const fs = require('node:fs');
        const nodePath = require('node:path');
        const response = await context.request.get(command.url, { timeout: 30000 });
        if (!response.ok()) {
          throw new Error(`HTTP ${response.status()} ${response.statusText()} for ${command.url}`);
        }
        const body = await response.body();
        const dir = nodePath.dirname(command.path);
        if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(command.path, body);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { path: command.path, size_bytes: body.length } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'save_file_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'switch_tab') {
      try {
        const idx = command.index === undefined ? -1 : command.index;
        const targetIdx = idx === -1 ? pages.length - 1 : idx;
        if (targetIdx < 0 || targetIdx >= pages.length || !pages[targetIdx]) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: `Invalid tab index ${idx}, have ${pages.length} tab(s)` } }) + '\n');
        } else {
          page = pages[targetIdx];
          await page.bringToFront();
          const url = page.url();
          const title = await page.title();
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url, title, tab_count: pages.length, pageIndex: targetIdx } }) + '\n');
        }
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'export_cookies') {
      try {
        const cookies = await context.cookies();
        let localStorage = {};
        try {
          localStorage = await page.evaluate(() => {
            const result = {};
            for (let i = 0; i < window.localStorage.length; i++) {
              const key = window.localStorage.key(i);
              if (key !== null) result[key] = window.localStorage.getItem(key) || '';
            }
            return result;
          });
        } catch (_) { /* localStorage may be unavailable on some pages */ }
        const url = page.url();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { cookies, local_storage: localStorage, url }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'export_cookies_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'import_cookies') {
      try {
        if (command.cookies && command.cookies.length > 0) {
          await context.addCookies(command.cookies);
        }
        if (command.local_storage && typeof command.local_storage === 'object') {
          await page.evaluate((ls) => {
            for (const [k, v] of Object.entries(ls)) {
              try { window.localStorage.setItem(k, v); } catch (_) {}
            }
          }, command.local_storage);
        }
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { imported: true }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'import_cookies_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    process.stdout.write(JSON.stringify({
      event: 'bridge_response',
      ok: false,
      error: { kind: 'unsupported_action', message: `Unsupported action: ${command.action}` }
    }) + '\n');
  }
}

bootstrap().catch((error) => {
  process.stdout.write(JSON.stringify({
    event: 'bridge_bootstrap',
    ok: false,
    error: { kind: 'launch_failed', message: String(error) }
  }) + '\n');
  process.exit(1);
});
"#;
