// Draws the title banner and the provider and MCP client logo grids used in the
// root README, in a light and a dark variant for GitHub's <picture> theme switch.
// GitHub strips inline SVG from READMEs, so they have to ship as image files.
// Run from docs/: `npm run readme-logos`.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { CLIENTS, PROVIDERS, type Brand } from '../src/lib/logos.ts';

const OUT = 'assets/readme';
const COLS = 6;
const CELL_W = 168;
const CELL_H = 44;
const GAP = 8;

const THEMES = {
	light: { logo: '#57606a', text: '#1f2328', border: '#d0d7de', accent: '#1f5fd6' },
	dark: { logo: '#9198a1', text: '#e6edf3', border: '#3d444d', accent: '#5b9dff' },
};

const escape = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');

function grid(brands: Brand[], theme: keyof typeof THEMES): string {
	const c = THEMES[theme];
	const rows = Math.ceil(brands.length / COLS);
	const width = COLS * CELL_W + (COLS - 1) * GAP;
	const height = rows * CELL_H + (rows - 1) * GAP;
	const cells = brands.map((brand, i) => {
		const x = (i % COLS) * (CELL_W + GAP);
		const y = Math.floor(i / COLS) * (CELL_H + GAP);
		const logo = brand.logo
			? `<svg x="${x + 14}" y="${y + 12}" width="20" height="20" viewBox="0 0 24 24" fill="${c.logo}">${brand.logo}</svg>`
			: '';
		const textX = brand.logo ? x + 42 : x + 16;
		return `<rect x="${x + 0.5}" y="${y + 0.5}" width="${CELL_W - 1}" height="${CELL_H - 1}" rx="10" fill="none" stroke="${c.border}"/>${logo}<text x="${textX}" y="${y + 27}" fill="${c.text}">${escape(brand.name)}</text>`;
	});
	return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans', Helvetica, Arial, sans-serif" font-size="14">${cells.join('')}</svg>\n`;
}

// Drawn cell by cell rather than as <text>, because box-drawing glyphs only line
// up in a monospace font and an SVG shown as an image can't rely on one.
const BANNER = [
	' █████╗  ██████╗██████╗  █████╗ ██╗    ██╗██╗     ',
	'██╔══██╗██╔════╝██╔══██╗██╔══██╗██║    ██║██║     ',
	'███████║██║     ██████╔╝███████║██║ █╗ ██║██║     ',
	'██╔══██║██║     ██╔══██╗██╔══██║██║███╗██║██║     ',
	'██║  ██║╚██████╗██║  ██║██║  ██║╚███╔███╔╝███████╗',
	'╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚══════╝',
];
const CHAR_W = 10;
const CHAR_H = 20;
const MARK_SIZE = 120;
const MARK_GAP = 28;

// [dx, dy] of the arms each double-line glyph has, away from the cell centre.
const ARMS: Record<string, [number, number][]> = {
	'═': [[-1, 0], [1, 0]],
	'║': [[0, -1], [0, 1]],
	'╔': [[1, 0], [0, 1]],
	'╗': [[-1, 0], [0, 1]],
	'╚': [[1, 0], [0, -1]],
	'╝': [[-1, 0], [0, -1]],
};

function doubleLine(glyph: string, x: number, y: number): string {
	const cx = x + CHAR_W / 2;
	const cy = y + CHAR_H / 2;
	const inset = 2.5;
	const arms = ARMS[glyph];
	const [hx, hy] = arms[0];
	const [vx, vy] = arms[1];
	if (arms.length === 2 && (hy === vy || hx === vx)) {
		const horizontal = hy === 0;
		return [-inset, inset]
			.map((d) =>
				horizontal
					? `M${x} ${cy + d}H${x + CHAR_W}`
					: `M${cx + d} ${y}V${y + CHAR_H}`,
			)
			.join('');
	}
	// A corner: the outer line bends around the inner one.
	const edgeX = hx > 0 ? x + CHAR_W : x;
	const edgeY = vy > 0 ? y + CHAR_H : y;
	return [-inset, inset]
		.map((d) => {
			const cornerX = cx + d * hx;
			const cornerY = cy + d * vy;
			return `M${edgeX} ${cornerY}H${cornerX}V${edgeY}`;
		})
		.join('');
}

function hero(theme: keyof typeof THEMES): string {
	const c = THEMES[theme];
	const mark = readFileSync('src/assets/mark.svg', 'utf8').replace(/^<svg[^>]*>/, '').replace(/<\/svg>\s*$/, '');
	const left = MARK_SIZE + MARK_GAP;
	const width = left + BANNER[0].length * CHAR_W;
	const height = BANNER.length * CHAR_H;
	let blocks = '';
	let lines = '';
	BANNER.forEach((row, r) => {
		[...row].forEach((glyph, i) => {
			const x = left + i * CHAR_W;
			const y = r * CHAR_H;
			if (glyph === '█') blocks += `M${x} ${y}h${CHAR_W}v${CHAR_H}h-${CHAR_W}z`;
			else if (glyph in ARMS) lines += doubleLine(glyph, x, y);
		});
	});
	return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><svg width="${MARK_SIZE}" height="${MARK_SIZE}" viewBox="0 0 64 64">${mark}</svg><path d="${lines}" fill="none" stroke="${c.accent}" stroke-width="1.5"/><path d="${blocks}" fill="${c.text}"/></svg>\n`;
}

mkdirSync(OUT, { recursive: true });
for (const theme of ['light', 'dark'] as const) {
	writeFileSync(`${OUT}/hero-${theme}.svg`, hero(theme));
}
for (const [name, brands] of [['providers', PROVIDERS], ['clients', CLIENTS]] as const) {
	for (const theme of ['light', 'dark'] as const) {
		writeFileSync(`${OUT}/${name}-${theme}.svg`, grid(brands, theme));
	}
}
console.log(`Wrote ${OUT}/{hero,providers,clients}-{light,dark}.svg`);
