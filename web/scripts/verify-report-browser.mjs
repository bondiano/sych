// Headless Chromium acceptance check for the offline HTML report (`--format html`):
// 1. Renders a hostile fixture (`</script>`, `<img src=x onerror=...>`, `javascript:` path, bidi
//    controls, and a 5,000-line source file) via `file://` and verifies zero network requests,
//    zero injected script execution, no `javascript:` anchors, no live bidi overrides, and total
//    file size within the 650 KiB budget.
// 2. Opens the committed `docs/examples/local-review/report.html` via `file://` and verifies it
//    renders offline with zero errors or network requests.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { chromium } from 'playwright-core'

const MAX_TEMPLATE_BYTES = 300 * 1024
const MAX_HOSTILE_REPORT_BYTES = 650 * 1024
const PLACEHOLDER = '<!--SYCH_REPORT_JSON-->'

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, '../..')
const templatePath = path.join(repoRoot, 'crates/sych-app/src/report_view/template.gen.html')
const exampleReportPath = path.join(repoRoot, 'docs/examples/local-review/report.html')

function findBrowserExecutable() {
  const candidates = [
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
    process.env.CHROME_BIN,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ].filter(Boolean)

  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate
    }
  }

  try {
    const pwPath = chromium.executablePath()
    if (pwPath && fs.existsSync(pwPath)) {
      return pwPath
    }
  } catch {
    // Fall through to explicit error when no browser binary is installed.
  }

  throw new Error(
    'No Chromium/Chrome binary found for headless report verification; install chromium or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH',
  )
}

function escapeScriptJson(value) {
  const raw = JSON.stringify(value)
  let out = ''
  for (const ch of raw) {
    const cp = ch.codePointAt(0) ?? 0
    if (ch === '<') {
      out += '\\u003c'
    } else if (
      cp === 0x2028 ||
      cp === 0x2029 ||
      (cp >= 0x202a && cp <= 0x202e) ||
      (cp >= 0x2066 && cp <= 0x2069) ||
      (cp >= 0x7f && cp <= 0x9f)
    ) {
      out += `\\u${cp.toString(16).padStart(4, '0')}`
    } else {
      out += ch
    }
  }
  return out
}

function buildFiveThousandLineFile() {
  const lines = []
  for (let i = 1; i <= 5000; i++) {
    if (i === 2500) {
      lines.push(`pub fn hostile_${i}() { let s = "</script><img src=x onerror=window.__pwn=1>"; }`)
    } else {
      lines.push(`pub fn item_${i}(x: u32) -> u32 { x.saturating_add(${i}) }`)
    }
  }
  return lines.join('\n') + '\n'
}

function buildHostileView() {
  const sha = 'sha1:' + 'a'.repeat(40)
  const snapId = 'sha256:' + 'b'.repeat(64)
  const digest = 'sha256:' + 'c'.repeat(64)
  const bigFile = buildFiveThousandLineFile()
  return {
    schema_version: 1,
    header: {
      run_id: '0192f000-0000-7000-8000-000000000001',
      result_id: '0192f000-0000-7000-8000-000000000002',
      review_revision: 1,
      snapshot: {
        schema_version: 1,
        id: snapId,
        repository: 'github:self:acme/hostile',
        target_ref: 'main',
        target_tip: sha,
        diff_base: sha,
        head: sha,
        head_ref: 'feat/hostile</script>',
        comparison: 'MERGE_BASE',
        source_manifest_hash: digest,
        diff_manifest_hash: digest,
        created_at: '2026-11-28T00:00:00Z',
      },
      code_links: {
        provider: 'github',
        base_url: 'https://github.com',
        repo: 'acme/hostile',
        head: 'a'.repeat(40),
        base: 'a'.repeat(40),
      },
      recommendation: 'ATTENTION_REQUIRED',
      coverage: 'PARTIAL',
      execution_outcome: 'PLAN_EXECUTED',
      counts: [{ severity: 'HIGH', category: 'DEFECT', count: 1 }],
      usage: {
        model_attempts: 0,
        input_tokens: 0,
        output_tokens: 0,
        reasoning_tokens: 0,
        unknown_usage_attempts: 0,
        cost: { known: 0, reference_estimate: 0, unknown_cost_attempts: 0 },
        routes: [],
      },
      language: 'en',
      created_at: '2026-11-28T00:00:00Z',
      provenance: { engine_version: '0.2.0', model_adapters: ['fake'] },
      scope: {
        revision: 1,
        units: [
          { id: '0192f000-0000-7000-8000-000000000003', title: 'hostile', paths: ['src/big.rs'] },
        ],
        required_obligation_ids: [],
        requested_profiles: ['security'],
        excluded: [],
      },
    },
    findings: [
      {
        id: '0192f000-0000-7000-8000-000000000004',
        category: 'DEFECT',
        severity: 'HIGH',
        lifecycle: 'OPEN',
        verification_status: 'CONFIRMED',
        support: 'STATIC_TRACE',
        title:
          'Hostile </script><script>window.__pwn="title"</script> <img src=x onerror="window.__pwn=\'img\'"> \u202e\u2066admin\u2069',
        explanation:
          'Body with `</script>` and <img src="https://evil.invalid/p.png" onerror="window.__pwn=\'body\'"> plus bidi \u202e.',
        triggering_conditions: ['when `<svg onload=window.__pwn=1>` renders'],
        consequence: 'must stay inert text',
        anchors: [
          {
            reference: {
              snapshot_id: snapId,
              side: 'HEAD',
              path: 'javascript:window.__pwn="href"',
              blob_oid: sha,
              start_line: 1,
              end_line: 1,
              content_sha256: digest,
            },
            excerpt: {
              first_line: 1,
              text: '<img src=x onerror=window.__pwn="excerpt">\n',
            },
            diff: '--- a/x.rs\n+++ b/x.rs\n@@ -1,1 +1,1 @@\n-old\n+</script><img src=x onerror=window.__pwn="diff">\n',
          },
          {
            reference: {
              snapshot_id: snapId,
              side: 'HEAD',
              path: 'src/big_\u202e.rs',
              blob_oid: sha,
              start_line: 2499,
              end_line: 2501,
              content_sha256: digest,
            },
            excerpt: {
              first_line: 1,
              text: bigFile,
            },
            diff: null,
          },
        ],
        also: [],
        suggested_action: 'Escape `<` as `\\u003c`',
        validation_expectation: 'No script runs and no network request fires.',
        limitations: [],
        rule_id: 'SEC-HIDDEN-TEXT',
        remediation: null,
        suggested_patch:
          'diff --git a/src/big.rs b/src/big.rs\n--- a/src/big.rs\n+++ b/src/big.rs\n@@ -1,1 +1,1 @@\n-bad\n+good\n',
        fix_note: null,
      },
    ],
    gaps: {
      unknown_obligations: [
        {
          id: '0192f000-0000-7000-8000-000000000005',
          rule_id: 'SEC-01',
          question: 'Does <img src=x onerror=window.__pwn=1> stay inert?',
          required: true,
          reason_code: 'NOT_ASSESSED',
          next_evidence_needed: 'headless check',
        },
      ],
      paths: [{ path: 'gen/hidden.rs', reason: 'GENERATED_IN_CHANGE' }],
      open_questions: ['Open <script>window.__pwn="gap"</script> question'],
      unresolved_candidates: [],
    },
  }
}

const template = fs.readFileSync(templatePath, 'utf8')
const templateBytes = Buffer.byteLength(template, 'utf8')
assert(
  templateBytes <= MAX_TEMPLATE_BYTES,
  `template size ${templateBytes} exceeds ${MAX_TEMPLATE_BYTES}`,
)
assert(template.includes(PLACEHOLDER), 'template must contain JSON placeholder')

const payload = escapeScriptJson(buildHostileView())
assert(!payload.includes('<'), 'escaped JSON payload must not contain raw <')
const parts = template.split(PLACEHOLDER)
assert.equal(parts.length, 2, 'placeholder must appear exactly once')
const hostileHtml = parts[0] + payload + parts[1]
const hostileBytes = Buffer.byteLength(hostileHtml, 'utf8')
assert(
  hostileBytes <= MAX_HOSTILE_REPORT_BYTES,
  `hostile 5000-line report is ${hostileBytes} bytes (${(hostileBytes / 1024).toFixed(1)} KiB), exceeding ${MAX_HOSTILE_REPORT_BYTES} bytes`,
)

const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'sych-report-test-'))
const hostilePath = path.join(tmpDir, 'hostile-report.html')
fs.writeFileSync(hostilePath, hostileHtml, 'utf8')

const executablePath = findBrowserExecutable()
const browser = await chromium.launch({ headless: true, executablePath })

try {
  // 1. Verify hostile 5,000-line fixture via file://
  {
    const context = await browser.newContext()
    const page = await context.newPage()
    const externalRequests = []
    const pageErrors = []
    let dialogOpened = false

    page.on('request', (req) => {
      const url = req.url()
      if (!url.startsWith('file://') && !url.startsWith('data:')) {
        externalRequests.push(url)
      }
    })
    page.on('pageerror', (err) => pageErrors.push(err.message))
    page.on('dialog', async (dialog) => {
      dialogOpened = true
      await dialog.dismiss()
    })

    await page.goto(pathToFileURL(hostilePath).href, { waitUntil: 'load' })
    await page.waitForSelector('main h1')

    const domCheck = await page.evaluate(() => {
      const pwn = window.__pwn
      const scripts = document.querySelectorAll('script').length
      const imgs = document.querySelectorAll('img').length
      const jsLinks = document.querySelectorAll('a[href^="javascript:"]').length
      const text = document.body.textContent ?? ''
      const hasRawBidi = /[\u202a-\u202e\u2066-\u2069]/.test(text)
      return { pwn, scripts, imgs, jsLinks, text, hasRawBidi }
    })

    assert.equal(dialogOpened, false, 'no dialog may open')
    assert.deepEqual(pageErrors, [], `unexpected page errors: ${pageErrors.join('; ')}`)
    assert.deepEqual(
      externalRequests,
      [],
      `offline report attempted network requests: ${externalRequests.join(', ')}`,
    )
    assert.equal(domCheck.pwn, undefined, `hostile script executed: ${domCheck.pwn}`)
    assert.equal(domCheck.scripts, 2, 'only the JSON payload tag and the bundle script may exist')
    assert.equal(domCheck.imgs, 0, 'hostile <img> markup must not create DOM img elements')
    assert.equal(domCheck.jsLinks, 0, 'javascript: paths must never render as clickable links')
    assert.equal(domCheck.hasRawBidi, false, 'raw bidi control characters must not reach DOM text')
    assert(domCheck.text.includes('U+202E'), 'bidi control must be shown as U+202E')
    assert(
      domCheck.text.includes('</script><script>window.__pwn="title"</script>'),
      'hostile script tag text must appear literally as inert text',
    )
    assert(
      domCheck.text.includes('Show all 5000 lines'),
      '5,000-line excerpt collapse button must render',
    )

    await page.click('button:has-text("Show all 5000 lines")')
    const expandedHasLast = await page.evaluate(() =>
      (document.body.textContent ?? '').includes('pub fn item_5000'),
    )
    assert(expandedHasLast, 'expanding the 5,000-line file must render line 5000')
    assert.deepEqual(pageErrors, [], 'expanding 5,000 lines must not throw')
    await context.close()
  }

  // 2. Verify committed docs/examples/local-review/report.html via file:// when present
  if (fs.existsSync(exampleReportPath)) {
    const context = await browser.newContext()
    const page = await context.newPage()
    const externalRequests = []
    const pageErrors = []

    page.on('request', (req) => {
      const url = req.url()
      if (!url.startsWith('file://') && !url.startsWith('data:')) {
        externalRequests.push(url)
      }
    })
    page.on('pageerror', (err) => pageErrors.push(err.message))

    await page.goto(pathToFileURL(exampleReportPath).href, { waitUntil: 'load' })
    await page.waitForSelector('main h1')
    const text = await page.evaluate(() => document.body.textContent ?? '')
    assert(text.includes('EFFECT-01'), 'example report must render EFFECT-01 finding')
    assert(text.includes('backend/orders/store.go'), 'example report must render file path')
    assert.deepEqual(pageErrors, [], `example report page errors: ${pageErrors.join('; ')}`)
    assert.deepEqual(
      externalRequests,
      [],
      `example report network requests: ${externalRequests.join(', ')}`,
    )
    await context.close()
  }

  console.log(
    `headless browser check ok (template ${(templateBytes / 1024).toFixed(1)} KiB, hostile 5000-line report ${(hostileBytes / 1024).toFixed(1)} KiB)`,
  )
} finally {
  await browser.close()
  fs.rmSync(tmpDir, { recursive: true, force: true })
}
