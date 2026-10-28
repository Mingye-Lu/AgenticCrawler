// Hero background: a web of pages that the crawler discovers outward from a
// seed, breadth-first, then fades and starts again from a new seed.

type Node = { x: number; y: number; r: number; visitedAt: number; links: number[] };

const VISIT_EVERY_MS = 70;
const EDGE_TRAVEL_MS = 420;
const HOLD_MS = 2600;

function css(name: string): string {
	return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function rand(seed: number): () => number {
	let s = seed >>> 0;
	return () => {
		s = (s * 1664525 + 1013904223) >>> 0;
		return s / 2 ** 32;
	};
}

function build(w: number, h: number, seed: number): Node[] {
	const rnd = rand(seed);
	const count = Math.round(Math.min(150, Math.max(50, (w * h) / 9000)));
	const nodes: Node[] = [];
	for (let i = 0; i < count; i++) {
		nodes.push({ x: rnd() * w, y: rnd() * h, r: 1.2 + rnd() * 2.2, visitedAt: -1, links: [] });
	}
	// Link each page to its three nearest neighbours.
	for (let i = 0; i < count; i++) {
		const near = nodes
			.map((n, j) => ({ j, d: (n.x - nodes[i].x) ** 2 + (n.y - nodes[i].y) ** 2 }))
			.filter((e) => e.j !== i)
			.sort((a, b) => a.d - b.d)
			.slice(0, 3);
		for (const { j } of near) {
			if (!nodes[i].links.includes(j)) nodes[i].links.push(j);
			if (!nodes[j].links.includes(i)) nodes[j].links.push(i);
		}
	}
	return nodes;
}

/** Breadth-first visit order from the node nearest (sx, sy), with each node's parent. */
function crawlOrder(nodes: Node[], sx: number, sy: number): { order: number[]; parent: number[] } {
	let seed = 0;
	let best = Infinity;
	nodes.forEach((n, i) => {
		const d = (n.x - sx) ** 2 + (n.y - sy) ** 2;
		if (d < best) [best, seed] = [d, i];
	});
	const parent = new Array(nodes.length).fill(-1);
	const seen = new Set([seed]);
	const order = [seed];
	for (let q = 0; q < order.length; q++) {
		for (const j of nodes[order[q]].links) {
			if (seen.has(j)) continue;
			seen.add(j);
			parent[j] = order[q];
			order.push(j);
		}
	}
	return { order, parent };
}

export function mountCrawlGraph(canvas: HTMLCanvasElement): void {
	const ctx = canvas.getContext('2d');
	if (!ctx) return;
	const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;

	let w = 0;
	let h = 0;
	let nodes: Node[] = [];
	let order: number[] = [];
	let parent: number[] = [];
	let start = 0;
	let round = 0;
	let frame = 0;
	let visible = true;

	const reset = (now: number) => {
		round++;
		nodes = build(w, h, 7 + round * 101);
		const seedX = w * (round % 2 ? 0.72 : 0.28);
		({ order, parent } = crawlOrder(nodes, seedX, h * 0.45));
		start = now;
	};

	const resize = () => {
		const dpr = Math.min(window.devicePixelRatio || 1, 2);
		const rect = canvas.getBoundingClientRect();
		w = rect.width;
		h = rect.height;
		canvas.width = Math.round(w * dpr);
		canvas.height = Math.round(h * dpr);
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		round = 0;
		reset(performance.now());
		if (reduced) draw(start + order.length * VISIT_EVERY_MS + EDGE_TRAVEL_MS);
	};

	const draw = (now: number) => {
		const t = now - start;
		const accent = css('--ac-accent');
		const spark = css('--ac-spark');
		const line = css('--ac-line-2');
		const crawlEnd = order.length * VISIT_EVERY_MS + EDGE_TRAVEL_MS;
		const fade = t > crawlEnd + HOLD_MS ? Math.max(0, 1 - (t - crawlEnd - HOLD_MS) / 900) : 1;
		if (!reduced && fade === 0) {
			reset(now);
			return;
		}

		ctx.clearRect(0, 0, w, h);
		ctx.globalAlpha = 1;

		// Undiscovered web.
		ctx.strokeStyle = line;
		ctx.lineWidth = 1;
		ctx.beginPath();
		nodes.forEach((n, i) => {
			for (const j of n.links) {
				if (j < i) continue;
				ctx.moveTo(n.x, n.y);
				ctx.lineTo(nodes[j].x, nodes[j].y);
			}
		});
		ctx.globalAlpha = 0.45;
		ctx.stroke();

		// Crawled edges, drawn growing from parent to child.
		ctx.globalAlpha = 0.85 * fade;
		ctx.strokeStyle = accent;
		ctx.lineWidth = 1.4;
		order.forEach((i, k) => {
			const p = parent[i];
			if (p < 0) return;
			const local = (t - k * VISIT_EVERY_MS) / EDGE_TRAVEL_MS;
			if (local <= 0) return;
			const f = Math.min(1, local);
			const a = nodes[p];
			const b = nodes[i];
			ctx.beginPath();
			ctx.moveTo(a.x, a.y);
			ctx.lineTo(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f);
			ctx.stroke();
		});

		// Pages: grey until visited, then blue with a short flash.
		for (const n of nodes) {
			ctx.globalAlpha = 0.5;
			ctx.fillStyle = line;
			ctx.beginPath();
			ctx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
			ctx.fill();
		}
		order.forEach((i, k) => {
			const arrived = t - (k * VISIT_EVERY_MS + EDGE_TRAVEL_MS * (k === 0 ? 0 : 1));
			if (arrived < 0) return;
			const n = nodes[i];
			const flash = Math.max(0, 1 - arrived / 600);
			ctx.globalAlpha = fade;
			ctx.fillStyle = k === 0 ? spark : accent;
			ctx.beginPath();
			ctx.arc(n.x, n.y, n.r + 0.8 + flash * 5, 0, Math.PI * 2);
			ctx.fill();
			if (flash > 0) {
				ctx.globalAlpha = flash * 0.35 * fade;
				ctx.beginPath();
				ctx.arc(n.x, n.y, n.r + 14 * (1 - flash) + 4, 0, Math.PI * 2);
				ctx.fill();
			}
		});
		ctx.globalAlpha = 1;
	};

	const loop = (now: number) => {
		if (visible) draw(now);
		frame = requestAnimationFrame(loop);
	};

	resize();
	new ResizeObserver(resize).observe(canvas);
	new IntersectionObserver(([e]) => (visible = e.isIntersecting)).observe(canvas);
	new MutationObserver(() => reduced && draw(start + order.length * VISIT_EVERY_MS + EDGE_TRAVEL_MS)).observe(
		document.documentElement,
		{ attributes: true, attributeFilter: ['data-theme'] },
	);
	if (!reduced) frame = requestAnimationFrame(loop);
	void frame;
}
