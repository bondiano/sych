// @ts-check
import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';
import { BASE, GROUPS, REPO } from './pages.mjs';

export default defineConfig({
	site: 'https://bondiano.github.io',
	base: BASE,
	integrations: [
		starlight({
			title: 'Sych',
			description: 'Deterministic code review: a Rust core collects evidence, models stay within bounds.',
			logo: {
				light: './src/assets/brand/lockup-transparent-dark.png',
				dark: './src/assets/brand/lockup-transparent-light.png',
				alt: 'Sych',
				replacesTitle: true,
			},
			favicon: '/brand/icon-transparent-32.png',
			social: [{ icon: 'github', label: 'GitHub', href: REPO }],
			customCss: ['@fontsource-variable/inter', './src/styles/brand.css'],
			sidebar: GROUPS.map((group) => ({
				label: group.label,
				items: [
					...group.pages.map(([, slug]) => slug),
					...(group.label === 'Reference'
						? [{ label: 'JSON Schemas', link: `${REPO}/tree/main/schemas` }]
						: []),
				],
			})),
		}),
	],
});
