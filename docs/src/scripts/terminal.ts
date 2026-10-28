// The hero terminal: types a goal, streams the agent's tool calls, then
// replays the saved script with `acrawl run`. Loops while on screen.

type Line = { kind: 'cmd' | 'out'; html: string; delay?: number };

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const tool = (name: string, arg: string, note: string) =>
	`<span class="t-arrow">→</span> <span class="t-tool">${name.padEnd(13)}</span> <span class="t-arg">${esc(arg.padEnd(26))}</span> <span class="t-note">${esc(note)}</span>`;

const SESSION: Line[] = [
	{ kind: 'cmd', html: 'acrawl prompt "every morning, grab the top 10 stories from news.example.com"' },
	{ kind: 'out', html: '<span class="t-plan">◆ plan</span>  <span class="t-dim">open the front page · read the story list · keep rank, title, url, points</span>', delay: 650 },
	{ kind: 'out', html: tool('navigate', 'news.example.com', 'http 200 · fit_markdown'), delay: 520 },
	{ kind: 'out', html: tool('page_map', 'main', '30 story rows'), delay: 420 },
	{ kind: 'out', html: tool('execute_js', 'rows.slice(0, 10)', '10 items'), delay: 460 },
	{ kind: 'out', html: tool('save_script', 'top_stories', '✓ saved'), delay: 480 },
	{ kind: 'out', html: '<span class="t-ok">✓ done</span> <span class="t-dim">· 4 tool calls · structured output below</span>', delay: 420 },
	{ kind: 'out', html: '<span class="t-dim">[{ "rank": 1, "title": "…", "url": "https://…", "points": 412 }, …]</span>', delay: 260 },
	{ kind: 'out', html: '', delay: 500 },
	{ kind: 'cmd', html: 'acrawl run top_stories --format jsonl -o today.jsonl' },
	{ kind: 'out', html: '<span class="t-dim">script completed: 10 item(s), 4 step(s), 2.8s</span>  <span class="t-zero">0 LLM calls</span>', delay: 700 },
];

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function mountTerminal(code: HTMLElement): void {
	const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
	const prompt = '<span class="t-prompt">$</span> ';

	if (reduced) {
		code.innerHTML = SESSION.map((l) => (l.kind === 'cmd' ? prompt + esc(l.html) : l.html)).join('\n');
		return;
	}

	let visible = false;
	new IntersectionObserver(([e]) => (visible = e.isIntersecting)).observe(code);

	const run = async () => {
		for (;;) {
			while (!visible) await sleep(300);
			const done: string[] = [];
			const render = (tail = '') => {
				code.innerHTML = [...done, tail].join('\n') + '<span class="t-caret"></span>';
			};
			for (const line of SESSION) {
				if (line.kind === 'cmd') {
					for (let i = 1; i <= line.html.length; i++) {
						render(prompt + esc(line.html.slice(0, i)));
						await sleep(18 + Math.random() * 30);
					}
					await sleep(380);
					done.push(prompt + esc(line.html));
				} else {
					await sleep(line.delay ?? 300);
					done.push(line.html);
				}
				render();
			}
			await sleep(5200);
		}
	};
	void run();
}
