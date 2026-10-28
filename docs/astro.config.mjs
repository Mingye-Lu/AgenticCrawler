// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// The site lives under a GitHub Pages project path. To move to a custom
// domain, change `site`, set `base` to '/', and update SITE in src/site.ts.
const site = 'https://mingye-lu.github.io';
const base = '/AgenticCrawler';

export default defineConfig({
	site,
	base,
	trailingSlash: 'always',
	integrations: [
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
			components: { Head: './src/components/DocsHead.astro' },
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
				{ label: 'Reference', items: ['docs/tools', 'docs/faq'] },
				{
					label: 'Comparisons',
					items: [
						'docs/comparisons/overview',
						'docs/comparisons/vs-browser-use',
						'docs/comparisons/vs-playwright-mcp',
						'docs/comparisons/vs-firecrawl',
					],
				},
			],
		}),
	],
});
