// mdBook-era URLs (e.g. /install.html), kept as real .html files that
// redirect, so links shared before the redesign still land on the page.
import type { APIRoute, GetStaticPaths } from 'astro';

const LEGACY: Record<string, string> = {
	'introduction.html': 'docs/',
	'install.html': 'docs/install/',
	'quickstart.html': 'docs/quickstart/',
	'mcp-server.html': 'docs/mcp-server/',
	'tools.html': 'docs/tools/',
	'faq.html': 'docs/faq/',
	'guides/scrape-js-sites.html': 'docs/guides/scrape-js-sites/',
	'guides/deterministic-scripts.html': 'docs/guides/deterministic-scripts/',
	'guides/devtools-debugging.html': 'docs/guides/devtools-debugging/',
	'guides/real-browser.html': 'docs/guides/real-browser/',
	'comparisons/overview.html': 'docs/comparisons/overview/',
	'comparisons/vs-browser-use.html': 'docs/comparisons/vs-browser-use/',
	'comparisons/vs-playwright-mcp.html': 'docs/comparisons/vs-playwright-mcp/',
	'comparisons/vs-firecrawl.html': 'docs/comparisons/vs-firecrawl/',
};

export const getStaticPaths = (() =>
	Object.entries(LEGACY).map(([legacy, to]) => ({ params: { legacy }, props: { to } }))) satisfies GetStaticPaths;

export const GET: APIRoute = ({ props, site }) => {
	const to = new URL(`${import.meta.env.BASE_URL}/${props.to}`.replace(/\/+/g, '/'), site).href;
	const html = `<!doctype html><meta charset="utf-8"><title>Moved</title><link rel="canonical" href="${to}"><meta name="robots" content="noindex"><meta http-equiv="refresh" content="0; url=${to}"><a href="${to}">${to}</a>`;
	return new Response(html, { headers: { 'Content-Type': 'text/html; charset=utf-8' } });
};
