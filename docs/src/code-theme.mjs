// Syntax theme for the docs' code blocks, matching the landing page's code
// panels: always dark, with blue for commands and keys, yellow for literals.
export const acrawlCodeTheme = {
	name: 'acrawl',
	type: 'dark',
	colors: {
		'editor.background': '#070a0f',
		'editor.foreground': '#e6ebf2',
	},
	tokenColors: [
		{ scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: '#7a879b' } },
		{ scope: ['string', 'string.quoted', 'markup.inline.raw'], settings: { foreground: '#ffe08a' } },
		{ scope: ['constant.numeric', 'constant.language', 'constant.character'], settings: { foreground: '#ffd23f' } },
		{
			scope: ['keyword', 'storage', 'entity.name.function', 'support.function', 'entity.name.command', 'support.function.builtin'],
			settings: { foreground: '#5b9dff' },
		},
		{
			scope: ['support.type.property-name', 'meta.object-literal.key', 'entity.name.tag', 'entity.other.attribute-name', 'constant.other.option', 'variable.parameter'],
			settings: { foreground: '#9cc4ff' },
		},
		{ scope: ['variable.other', 'variable.language'], settings: { foreground: '#e6ebf2' } },
		// Shell arguments are plain text, as on the landing page.
		{ scope: ['string.unquoted.argument.shell', 'string.unquoted.argument'], settings: { foreground: '#e6ebf2' } },
		{ scope: ['punctuation', 'meta.brace', 'keyword.operator'], settings: { foreground: '#93a0b4' } },
	],
};
