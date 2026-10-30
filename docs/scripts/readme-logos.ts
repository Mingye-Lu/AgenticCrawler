// Draws the provider and MCP client logo grids used in the root README, in a
// light and a dark variant for GitHub's <picture> theme switch. GitHub strips
// inline SVG from READMEs, so the logos have to ship as image files.
// Run from docs/: `npm run readme-logos`.
import { mkdirSync, writeFileSync } from 'node:fs';
import { CLIENTS, PROVIDERS, type Brand } from '../src/lib/logos.ts';

const OUT = 'assets/readme';
const COLS = 6;
const CELL_W = 168;
const CELL_H = 44;
const GAP = 8;

const THEMES = {
	light: { logo: '#57606a', text: '#1f2328', border: '#d0d7de' },
	dark: { logo: '#9198a1', text: '#e6edf3', border: '#3d444d' },
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

mkdirSync(OUT, { recursive: true });
for (const [name, brands] of [['providers', PROVIDERS], ['clients', CLIENTS]] as const) {
	for (const theme of ['light', 'dark'] as const) {
		writeFileSync(`${OUT}/${name}-${theme}.svg`, grid(brands, theme));
	}
}
console.log(`Wrote ${OUT}/{providers,clients}-{light,dark}.svg`);
