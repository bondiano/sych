// Builds the single-file offline HTML report template from `vite.report.config.ts` and either
// writes `../crates/sych-app/src/report_view/template.gen.html` (`--write`) or checks that the
// committed template is byte-identical and within the 300 KiB size budget (`--check`).
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'vite'

const MAX_TEMPLATE_BYTES = 300 * 1024
const PLACEHOLDER = '<!--SYCH_REPORT_JSON-->'

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const webRoot = path.resolve(scriptDir, '..')
const outDir = path.join(webRoot, 'dist-report')
const targetPath = path.resolve(webRoot, '../crates/sych-app/src/report_view/template.gen.html')

const mode = process.argv.includes('--check') ? 'check' : 'write'

await build({
  configFile: path.join(webRoot, 'vite.report.config.ts'),
  root: webRoot,
  logLevel: 'warn',
})

const htmlShell = fs.readFileSync(path.join(outDir, 'report.html'), 'utf8')
const jsPath = path.join(outDir, 'report.js')
const cssPath = path.join(outDir, 'report.css')

const rawJs = fs.readFileSync(jsPath, 'utf8')
const rawCss = fs.existsSync(cssPath) ? fs.readFileSync(cssPath, 'utf8') : ''

// Escape any literal `</script` or `</style` sequence inside bundled regexes/strings so the HTML5
// tokenizer never closes the inline element early.
const safeJs = rawJs.replace(/<\/(script)/gi, '<\\/$1')
const safeCss = rawCss.replace(/<\/(style)/gi, '<\\/$1')

const FORBIDDEN_SNIPPETS = ['vue-router', '@tanstack/vue-query', 'openapi-fetch', 'sych_session']
for (const forbidden of FORBIDDEN_SNIPPETS) {
  if (safeJs.includes(forbidden)) {
    throw new Error(`offline report bundle unexpectedly includes "${forbidden}"`)
  }
}

let assembled = htmlShell
  .replace(/<link[^>]+rel="stylesheet"[^>]*>\s*/g, '')
  .replace(/<link[^>]+rel="modulepreload"[^>]*>\s*/g, '')
  .replace(/<script[^>]+src="[^"]*report\.js"[^>]*><\/script>\s*/g, '')

if (!assembled.includes(PLACEHOLDER)) {
  throw new Error(`report.html is missing ${PLACEHOLDER}`)
}

if (!assembled.includes("default-src 'none'")) {
  throw new Error('report.html is missing the offline Content-Security-Policy meta tag')
}

if (safeCss) {
  assembled = assembled.replace(
    '</head>',
    () => `    <style>\n${safeCss.trim()}\n    </style>\n  </head>`,
  )
}

assembled = assembled.replace(
  '</body>',
  () => `    <script>\n${safeJs.trim()}\n    </script>\n  </body>`,
)

if (/<script[^>]+src=/i.test(assembled) || /<link[^>]+href=/i.test(assembled)) {
  throw new Error('assembled template still contains an external script or stylesheet reference')
}

const normalized = assembled.replace(/\r\n/g, '\n').trimEnd() + '\n'
const byteLength = Buffer.byteLength(normalized, 'utf8')
if (byteLength > MAX_TEMPLATE_BYTES) {
  throw new Error(
    `template.gen.html is ${byteLength} bytes (${(byteLength / 1024).toFixed(1)} KiB), exceeding the ${MAX_TEMPLATE_BYTES} byte (300 KiB) budget`,
  )
}

fs.rmSync(outDir, { recursive: true, force: true })

if (mode === 'check') {
  const committed = fs.existsSync(targetPath) ? fs.readFileSync(targetPath, 'utf8') : ''
  if (committed !== normalized) {
    throw new Error(
      `${targetPath} is stale; run \`just report-template\` (built ${(byteLength / 1024).toFixed(1)} KiB)`,
    )
  }

  console.log(`report template ok (${(byteLength / 1024).toFixed(1)} KiB <= 300.0 KiB)`)
} else {
  fs.writeFileSync(targetPath, normalized, 'utf8')
  console.log(`wrote ${targetPath} (${(byteLength / 1024).toFixed(1)} KiB)`)
}
