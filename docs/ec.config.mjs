// Expressive Code (Starlight's code blocks). Kept out of astro.config.mjs
// because a custom theme object there is hashed differently by the page and
// the stylesheet builds, so pages link a stylesheet that doesn't exist.
import { defineEcConfig } from '@astrojs/starlight/expressive-code';
import { acrawlCodeTheme } from './src/code-theme.mjs';

// Code blocks match the landing page's code panels in both themes.
export default defineEcConfig({
	themes: [acrawlCodeTheme],
	styleOverrides: {
		borderRadius: '0.9rem',
		borderColor: 'var(--ac-line-2)',
		codeFontFamily: 'var(--ac-mono)',
		codeFontSize: '0.85rem',
		codeBackground: '#070a0f',
		frames: {
			frameBoxShadowCssValue: 'none',
			terminalBackground: '#070a0f',
			terminalTitlebarBackground: '#0d121a',
			terminalTitlebarBorderBottomColor: '#1a2230',
			terminalTitlebarDotsForeground: '#3a4a62',
			editorTabBarBackground: '#0d121a',
			editorActiveTabBackground: '#070a0f',
			editorTabBarBorderBottomColor: '#1a2230',
		},
	},
});
