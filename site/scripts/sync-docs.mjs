// Copies the published docs/ pages into Starlight's content directory: the H1 becomes the title,
// links to other pages become site URLs, other repository links point at GitHub, and the brand
// images a page shows are copied next to the site. A link to a missing file fails the build.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE, PAGES, REPO } from '../pages.mjs';

const LINK = /(\]\(|(?:src|srcset|href)=")([^)"\s]+)/g;

/** Rewrites one page; `exists(repoPath)` returns 'file', 'dir' or null. */
export function rewrite(text, source, exists) {
	const [first, ...rest] = text.split('\n');
	const title = first.match(/^# (.+)$/)?.[1];
	if (!title) throw new Error(`${source}: the first line must be the H1 title`);
	const images = new Set();
	let fence = false;
	const body = rest
		.map((line) => {
			if (/^\s*```/.test(line)) fence = !fence;
			if (fence) return line;
			return line.replace(LINK, (_, open, target) => open + url(target, source, exists, images));
		})
		.join('\n');
	const front = `---\ntitle: ${JSON.stringify(title)}\neditUrl: ${REPO}/edit/main/${source}\n---\n`;
	return { text: front + body, images };
}

function url(target, source, exists, images) {
	if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) return target;
	const [file, anchor] = target.split('#');
	const repoPath = path.posix.normalize(path.posix.join(path.posix.dirname(source), file)).replace(/\/$/, '');
	const hash = anchor === undefined ? '' : `#${anchor}`;
	const kind = exists(repoPath);
	if (!kind) throw new Error(`${source}: link to missing ${target}`);
	if (PAGES.has(repoPath)) return `${BASE}${PAGES.get(repoPath) && `${PAGES.get(repoPath)}/`}${hash}`;
	if (repoPath.startsWith('docs/brand/') && repoPath.endsWith('.png')) {
		images.add(repoPath);
		return `${BASE}brand/${path.posix.basename(repoPath)}`;
	}
	return `${REPO}/${kind === 'dir' ? 'tree' : 'blob'}/main/${repoPath}${hash}`;
}

function main() {
	const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
	const root = path.resolve(site, '..');
	const content = path.join(site, 'src/content/docs');
	const exists = (p) => {
		const stat = fs.statSync(path.join(root, p), { throwIfNoEntry: false });
		return stat ? (stat.isDirectory() ? 'dir' : 'file') : null;
	};
	for (const entry of fs.readdirSync(content)) {
		if (entry !== 'index.mdx') fs.rmSync(path.join(content, entry), { recursive: true });
	}
	const images = new Set(['docs/brand/icon-transparent-32.png']);
	for (const [source, slug] of PAGES) {
		if (!slug) continue;
		const page = rewrite(fs.readFileSync(path.join(root, source), 'utf8'), source, exists);
		const out = path.join(content, `${slug}.md`);
		fs.mkdirSync(path.dirname(out), { recursive: true });
		fs.writeFileSync(out, page.text);
		page.images.forEach((i) => images.add(i));
	}
	const copy = (from, dir) => {
		fs.mkdirSync(dir, { recursive: true });
		fs.copyFileSync(path.join(root, from), path.join(dir, path.basename(from)));
	};
	images.forEach((i) => copy(i, path.join(site, 'public/brand')));
	// Imported by the config and the home page, so Astro optimizes them.
	for (const name of ['lockup-transparent-dark.png', 'lockup-transparent-light.png', 'symbol-transparent.png']) {
		copy(`docs/brand/${name}`, path.join(site, 'src/assets/brand'));
	}
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
