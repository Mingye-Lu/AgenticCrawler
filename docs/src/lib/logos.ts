// Brand marks for the MCP clients and LLM providers shown on the landing page,
// read at build time from LobeHub's AI icon set and simple-icons. Each entry is
// the inner markup of a 24×24 single-colour SVG drawn in `currentColor`.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { siGitlab, siJetbrains, siZedindustries } from 'simple-icons';

const LOBE_DIR = join(process.cwd(), 'node_modules/@lobehub/icons-static-svg/icons');

function lobe(name: string): string {
	return readFileSync(join(LOBE_DIR, `${name}.svg`), 'utf8')
		.replace(/^<svg[^>]*>/, '')
		.replace(/<\/svg>\s*$/, '')
		.replace(/<title>.*?<\/title>/, '');
}

const simple = (icon: { path: string }) => `<path d="${icon.path}"/>`;

export interface Brand {
	name: string;
	logo?: string;
}

// VS Code, Crush and Aider have no mark in either set, so they show as text.
export const CLIENTS: Brand[] = [
	{ name: 'Claude Code', logo: lobe('claudecode') },
	{ name: 'Claude Desktop', logo: lobe('claude') },
	{ name: 'Cursor', logo: lobe('cursor') },
	{ name: 'Windsurf', logo: lobe('windsurf') },
	{ name: 'VS Code' },
	{ name: 'OpenCode', logo: lobe('opencode') },
	{ name: 'Zed', logo: simple(siZedindustries) },
	{ name: 'TRAE', logo: lobe('trae') },
	{ name: 'JetBrains', logo: simple(siJetbrains) },
	{ name: 'Gemini CLI', logo: lobe('geminicli') },
	{ name: 'Qwen Code', logo: lobe('qwen') },
	{ name: 'Codex CLI', logo: lobe('codex') },
	{ name: 'Hermes', logo: lobe('hermesagent') },
	{ name: 'OpenClaw', logo: lobe('openclaw') },
	{ name: 'Goose', logo: lobe('goose') },
	{ name: 'Crush' },
	{ name: 'Aider' },
];

export const PROVIDERS: Brand[] = [
	{ name: 'Anthropic', logo: lobe('anthropic') },
	{ name: 'OpenAI', logo: lobe('openai') },
	{ name: 'Google Gemini', logo: lobe('gemini') },
	{ name: 'DeepSeek', logo: lobe('deepseek') },
	{ name: 'Amazon Bedrock', logo: lobe('bedrock') },
	{ name: 'Azure OpenAI', logo: lobe('azure') },
	{ name: 'Vertex AI', logo: lobe('vertexai') },
	{ name: 'GitHub Copilot', logo: lobe('githubcopilot') },
	{ name: 'GitLab Duo', logo: simple(siGitlab) },
	{ name: 'Groq', logo: lobe('groq') },
	{ name: 'Cerebras', logo: lobe('cerebras') },
	{ name: 'Mistral AI', logo: lobe('mistral') },
	{ name: 'xAI', logo: lobe('xai') },
	{ name: 'Cohere', logo: lobe('cohere') },
	{ name: 'Alibaba', logo: lobe('alibaba') },
	{ name: 'OpenRouter', logo: lobe('openrouter') },
	{ name: 'Together AI', logo: lobe('together') },
	{ name: 'Ollama', logo: lobe('ollama') },
];
