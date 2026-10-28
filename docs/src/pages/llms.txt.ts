// llms.txt (https://llmstxt.org): a map of the docs for AI assistants,
// generated from the docs collection so it never drifts from the sidebar.
import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { DESCRIPTION, FACTS, REPO, SITE } from '../site';
import { docUrl, sortDocs } from '../lib/docs';

const SECTIONS: [string, (id: string) => boolean][] = [
	['Docs', (id) => !id.includes('/guides/') && !id.includes('/comparisons/')],
	['Guides', (id) => id.includes('/guides/')],
	['Comparisons', (id) => id.includes('/comparisons/')],
];

export const GET: APIRoute = async () => {
	const docs = sortDocs(await getCollection('docs'));
	const lines = [
		'# acrawl (AgenticCrawler)',
		'',
		`> ${DESCRIPTION}`,
		'',
		`Key facts: MIT-licensed, written in Rust. The agent has ${FACTS.agentTools} tools; the MCP server (\`acrawl mcp\`, stdio) exposes ${FACTS.mcpTools}: 32 browser and DevTools tools, 7 script tools and \`run_goal\`. It supports ${FACTS.providers} LLM providers plus any OpenAI-compatible endpoint, installs into ${FACTS.mcpClients} MCP clients with \`acrawl mcp install\`, and replays saved JSON scripts with no LLM via \`acrawl run\`. Browser automation uses a bundled stealth browser (CloakBrowser, needs Node.js 20+) or the user's own browser through the acrawl Bridge extension. Source: ${REPO}`,
	];
	for (const [title, match] of SECTIONS) {
		lines.push('', `## ${title}`, '');
		for (const d of docs.filter((d) => match(d.id))) {
			lines.push(`- [${d.data.title}](${docUrl(d.id)}): ${d.data.description}`);
		}
	}
	lines.push(
		'',
		'## Optional',
		'',
		`- [Full docs as one text file](${SITE}llms-full.txt)`,
		`- [README](${REPO}/blob/main/README.md)`,
		`- [Changelog](${REPO}/blob/main/CHANGELOG.md)`,
		'',
	);
	return new Response(lines.join('\n'), { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
};
