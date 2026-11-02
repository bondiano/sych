// The published pages: one list for the sync script and the sidebar.
export const REPO = 'https://github.com/bondiano/sych';
export const BASE = '/sych/';

export const GROUPS = [
	{
		label: 'Start',
		pages: [
			['docs/INSTALL.md', 'install'],
			['docs/LOCAL_REVIEW.md', 'local-review'],
		],
	},
	{
		label: 'Configure',
		pages: [
			['docs/CONFIG.md', 'config'],
			['docs/MODELS.md', 'models'],
			['docs/DEPLOYMENT.md', 'deployment'],
		],
	},
	{
		label: 'Run a server',
		pages: [
			['docs/SERVER.md', 'server'],
			['docs/STORAGE.md', 'storage'],
		],
	},
	{
		label: 'How it works',
		pages: [
			['docs/ANALYSIS.md', 'analysis'],
			['docs/SEMANTIC.md', 'semantic'],
		],
	},
	{
		label: 'Reference',
		pages: [
			['docs/adr/0001-execution-outcome-values.md', 'adr/0001-execution-outcome-values'],
			['docs/brand/README.md', 'brand'],
		],
	},
];

// Repository path → site slug; the docs index is the site's home page.
export const PAGES = new Map([['docs/README.md', ''], ...GROUPS.flatMap((g) => g.pages)]);
