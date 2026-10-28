// Every docs page as Markdown in one file, for assistants that want the
// whole picture in a single fetch.
import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { docUrl, sortDocs } from '../lib/docs';

export const GET: APIRoute = async () => {
	const docs = sortDocs(await getCollection('docs'));
	const parts = docs.map((d) => {
		const body = (d.body ?? '')
			.replace(/<video[\s\S]*?<\/video>\s*/g, '')
			.replace(/<script[\s\S]*?<\/script>\s*/g, '')
			.trim();
		return `<!-- ${docUrl(d.id)} -->\n\n# ${d.data.title}\n\n${body}\n`;
	});
	return new Response(parts.join('\n\n---\n\n'), {
		headers: { 'Content-Type': 'text/plain; charset=utf-8' },
	});
};
