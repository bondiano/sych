import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';
import { rewrite } from './sync-docs.mjs';

const tree = { 'docs/SERVER.md': 'file', 'docs/README.md': 'file', 'docs/brand/logo.png': 'file', 'schemas': 'dir', 'AGENTS.md': 'file' };
const exists = (p) => tree[p] ?? null;
const page = (body) => rewrite(`# Config: files\n${body}`, 'docs/CONFIG.md', exists);

test('the H1 becomes the frontmatter title', () => {
	assert.match(page('text').text, /^---\ntitle: "Config: files"\neditUrl: https:\/\/github\.com\/bondiano\/sych\/edit\/main\/docs\/CONFIG\.md\n---\ntext$/);
});

test('a published page becomes a site URL and keeps its anchor', () => {
	assert.match(page('[s](SERVER.md#self-review-bondianosych) [i](README.md)').text, /\(\/sych\/server\/#self-review-bondianosych\) \[i\]\(\/sych\/\)/);
});

test('other repository paths point at GitHub', () => {
	const { text } = page('[a](../AGENTS.md#rules) [s](../schemas/)');
	assert.match(text, /\(https:\/\/github\.com\/bondiano\/sych\/blob\/main\/AGENTS\.md#rules\)/);
	assert.match(text, /\(https:\/\/github\.com\/bondiano\/sych\/tree\/main\/schemas\)/);
});

test('brand images are copied and served from the site', () => {
	const { text, images } = page('<img src="brand/logo.png" />');
	assert.match(text, /src="\/sych\/brand\/logo\.png"/);
	assert.deepEqual([...images], ['docs/brand/logo.png']);
});

test('external links, anchors and code blocks stay as they are', () => {
	const body = '[x](https://example.com) [y](#here)\n```md\n[z](MISSING.md)\n```';
	assert.equal(page(body).text.split('---\n')[2], body);
});

test('a link to a missing file fails', () => {
	assert.throws(() => page('[m](MISSING.md)'), /link to missing MISSING\.md/);
});

test('the README shows the first install prompt word for word', () => {
	const read = (p) => fs.readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
	const prompt = read('docs/INSTALL.md').match(/```text\n[\s\S]*?\n```/)[0];
	assert.ok(read('README.md').includes(prompt), 'copy the first prompt of docs/INSTALL.md into README.md');
});
