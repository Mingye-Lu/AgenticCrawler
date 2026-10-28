// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';

// The site lives under a GitHub Pages project path. To move to a custom
// domain, change `site`, set `base` to '/', and update SITE in src/site.ts.
const site = 'https://mingye-lu.github.io';
const base = '/AgenticCrawler';

// The source file behind a sitemap URL, so <lastmod> can come from git.
function sourceFor(url) {
	const path = new URL(url).pathname.slice(base.length).replace(/^\/|\/$/g, '');
	if (path === '') return 'src/pages/index.astro';
	for (const candidate of [`src/content/docs/${path}.md`, `src/content/docs/${path}/index.md`]) {
		if (existsSync(candidate)) return candidate;
	}
	return null;
}

// Last commit date of a file. Needs full history (the Pages workflow fetches it).
function lastCommitDate(file) {
	try {
		const date = execFileSync('git', ['log', '-1', '--format=%cI', '--', file], { encoding: 'utf8' }).trim();
		return date || undefined;
	} catch {
		return undefined;
	}
}

export default defineConfig({
	site,
	base,
	trailingSlash: 'always',
	integrations: [
		// Declared here so Starlight skips its own sitemap and this one can add <lastmod>.
		sitemap({
			serialize(item) {
				const file = sourceFor(item.url);
				const lastmod = file && lastCommitDate(file);
				return lastmod ? { ...item, lastmod } : item;
			},
		}),
		starlight({
			title: 'acrawl',
			description:
				'acrawl is an open-source browser agent and MCP server in one Rust binary. Describe a web task in plain English and get structured data back.',
			logo: { src: './src/assets/mark.svg', alt: '' },
			favicon: '/favicon.svg',
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/Mingye-Lu/AgenticCrawler' },
			],
			editLink: { baseUrl: 'https://github.com/Mingye-Lu/AgenticCrawler/edit/main/docs/' },
			lastUpdated: true,
			customCss: [
				'@fontsource-variable/space-grotesk',
				'@fontsource-variable/jetbrains-mono',
				'./src/styles/tokens.css',
				'./src/styles/docs.css',
			],
			components: {
				Head: './src/components/DocsHead.astro',
				SiteTitle: './src/components/SiteTitle.astro',
				SocialIcons: './src/components/SocialIcons.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
			},
			head: [
				{ tag: 'meta', attrs: { property: 'og:image', content: `${site}${base}/media/og.jpg` } },
				{ tag: 'meta', attrs: { property: 'og:image:width', content: '1200' } },
				{ tag: 'meta', attrs: { property: 'og:image:height', content: '630' } },
				{ tag: 'meta', attrs: { name: 'twitter:image', content: `${site}${base}/media/og.jpg` } },
				{
					tag: 'link',
					attrs: { rel: 'alternate', type: 'text/plain', title: 'llms.txt', href: `${base}/llms.txt` },
				},
			],
			sidebar: [
				{
					label: 'Getting started',
					items: [
						{ label: 'What is acrawl?', link: '/docs/' },
						'docs/install',
						'docs/quickstart',
						'docs/mcp-server',
						'docs/providers',
					],
				},
				{
					label: 'Guides',
					items: [
						'docs/guides/scrape-js-sites',
						'docs/guides/deterministic-scripts',
						'docs/guides/devtools-debugging',
						'docs/guides/real-browser',
					],
				},
				{
					label: 'Reference',
					items: ['docs/how-it-works', 'docs/tools', 'docs/cli', 'docs/configuration', 'docs/faq'],
				},
				{
					label: 'Comparisons',
					items: [
						'docs/comparisons/overview',
						'docs/comparisons/vs-browser-use',
						'docs/comparisons/vs-playwright-mcp',
						'docs/comparisons/vs-firecrawl',
					],
				},
				{ label: 'Project', items: ['about'] },
			],
		}),
	],
});
