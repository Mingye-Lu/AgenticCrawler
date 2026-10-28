import type { CollectionEntry } from 'astro:content';
import { SITE } from '../site';

// Sidebar order, so llms.txt and llms-full.txt read like the docs do.
const ORDER = [
	'docs',
	'docs/install',
	'docs/quickstart',
	'docs/mcp-server',
	'docs/providers',
	'docs/guides/scrape-js-sites',
	'docs/guides/deterministic-scripts',
	'docs/guides/devtools-debugging',
	'docs/guides/real-browser',
	'docs/how-it-works',
	'docs/tools',
	'docs/cli',
	'docs/configuration',
	'docs/faq',
	'docs/comparisons/overview',
	'docs/comparisons/vs-browser-use',
	'docs/comparisons/vs-playwright-mcp',
	'docs/comparisons/vs-firecrawl',
];

export function docUrl(id: string): string {
	return new URL(`${id}/`, SITE).href;
}

export function sortDocs(docs: CollectionEntry<'docs'>[]): CollectionEntry<'docs'>[] {
	const rank = (id: string) => {
		const i = ORDER.indexOf(id);
		return i === -1 ? ORDER.length : i;
	};
	return [...docs].sort((a, b) => rank(a.id) - rank(b.id) || a.id.localeCompare(b.id));
}
