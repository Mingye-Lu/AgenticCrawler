// Facts shared by the landing page, structured data and llms.txt. Keep
// these in step with the README and AGENTS.md when the numbers change.
import cargo from '../../Cargo.toml?raw';

export const SITE = 'https://mingye-lu.github.io/AgenticCrawler/';
export const REPO = 'https://github.com/Mingye-Lu/AgenticCrawler';
export const VERSION = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1] ?? '';

export const TAGLINE = 'Describe it. acrawl it.';
export const DESCRIPTION =
	'acrawl is an open-source browser agent and MCP server in one Rust binary. Describe a web task in plain English and it navigates, clicks, fills forms, watches the network and returns structured data.';

// Search snippets are cut at about 155 characters; keep this under that.
export const META_DESCRIPTION =
	'Open-source AI browser agent and MCP server in one Rust binary. Describe a web task in plain English and get structured data back, with DevTools built in.';

export const FACTS = {
	agentTools: 43,
	mcpTools: 40,
	providers: 25,
	mcpClients: 17,
	platforms: 5,
};

export const INSTALL = {
	unix: 'curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash',
	windows: 'irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex',
	source: 'git clone https://github.com/Mingye-Lu/AgenticCrawler.git && cd AgenticCrawler && cargo build --release',
};
