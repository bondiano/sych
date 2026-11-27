import { describe, expect, it } from 'vitest'

import type { Schemas } from '@/modules/Api'

import { parseUnifiedDiff } from '../diff'
import { highlightLines } from '../highlight'
import {
  agentPrompt,
  categoryLabel,
  codeLink,
  formatSeconds,
  hex,
  optional,
  place,
  problems,
  proseParts,
  safeText,
  verdict,
} from '../report'

const anchor = (over: Partial<Schemas['CodeRef']> = {}) =>
  ({ path: 'src/a.ts', side: 'HEAD', start_line: 3, end_line: 5, ...over }) as Schemas['CodeRef']

const finding = (over: Partial<Schemas['Finding']>) =>
  ({
    category: 'DEFECT',
    severity: 'MEDIUM',
    fingerprint: 'a',
    title: 'Title',
    explanation: 'Why.',
    anchors: [anchor()],
    suggested_action: '',
    validation_expectation: '',
    ...over,
  }) as Schemas['Finding']

const result = (findings: Schemas['Finding'][], coverage: Schemas['Coverage']) =>
  ({ findings, coverage }) as Schemas['ReviewResult']

describe('verdict', () => {
  it('never reads as "no problems" when coverage is partial', () => {
    const v = verdict(result([], 'PARTIAL'))
    expect(v.tone).toBe('warn')
    expect(v.title).toBe('None confirmed in what could be checked')
    expect(v.coverage).toContain('not "no problems"')
  })

  it('is fine only with complete coverage and no problem', () => {
    expect(verdict(result([], 'COMPLETE_FOR_SCOPE')).tone).toBe('ok')
    expect(verdict(result([finding({ category: 'NIT' })], 'COMPLETE_FOR_SCOPE')).tone).toBe('ok')
    expect(verdict(result([finding({})], 'COMPLETE_FOR_SCOPE')).tone).toBe('warn')
  })

  it('counts high and critical apart and localizes to Russian', () => {
    const r = result([finding({ severity: 'CRITICAL' }), finding({})], 'COMPLETE_FOR_SCOPE')
    expect(verdict(r)).toMatchObject({ tone: 'bad', title: '2 problems, 1 high or critical' })
    expect(verdict(r, 'ru').title).toContain('Проблем: 2')
    expect(categoryLabel('RULE_VIOLATION', 'ru')).toBe('Нарушение правила')
  })
})

describe('problems and optional', () => {
  it('splits by category and orders by severity, then fingerprint', () => {
    const r = result(
      [
        finding({ severity: 'LOW', fingerprint: 'a' }),
        finding({ severity: 'HIGH', fingerprint: 'b' }),
        finding({ severity: 'HIGH', fingerprint: 'a', category: 'RULE_VIOLATION' }),
        finding({ category: 'IMPROVEMENT' }),
      ],
      'PARTIAL',
    )
    expect(problems(r).map((f) => [f.severity, f.fingerprint])).toEqual([
      ['HIGH', 'a'],
      ['HIGH', 'b'],
      ['LOW', 'a'],
    ])
    expect(optional(r).map((f) => f.category)).toEqual(['IMPROVEMENT'])
  })
})

describe('code links', () => {
  const links = { repo: 'acme/shop', head: 'bbb', base: 'aaa' }

  it('points at the commit of the side it names on GitHub and GitLab', () => {
    expect(codeLink(anchor(), links)).toBe('https://github.com/acme/shop/blob/bbb/src/a.ts#L3-L5')
    expect(codeLink(anchor({ side: 'DIFF_BASE', end_line: 3 }), links)).toBe(
      'https://github.com/acme/shop/blob/aaa/src/a.ts#L3',
    )
    expect(
      codeLink(anchor(), {
        provider: 'gitlab',
        base_url: 'https://gitlab.example.com',
        repo: 'group/sub/shop',
        head: 'bbb',
        base: 'aaa',
      }),
    ).toBe('https://gitlab.example.com/group/sub/shop/-/blob/bbb/src/a.ts#L3-5')
  })

  it('refuses missing snapshots, target tip, javascript: paths and non-HTTP origins', () => {
    expect(codeLink(anchor(), undefined)).toBeUndefined()
    expect(codeLink(anchor({ side: 'TARGET_TIP' }), links)).toBeUndefined()
    expect(codeLink(anchor({ path: 'javascript:alert(1)' }), links)).toBeUndefined()
    expect(
      codeLink(anchor(), {
        provider: 'github',
        base_url: 'javascript:alert(1)',
        repo: 'acme/shop',
        head: 'bbb',
        base: 'aaa',
      }),
    ).toBeUndefined()
  })

  it('names a place, neutralizes bidi controls, and strips the object id prefix', () => {
    expect(place(anchor())).toBe('src/a.ts:3-5')
    expect(place(anchor({ end_line: 3 }))).toBe('src/a.ts:3')
    expect(safeText('a\u202eb')).toBe('aU+202Eb')
    expect(hex('sha1:abc')).toBe('abc')
  })
})

describe('agentPrompt', () => {
  it('words each task as the report does and leaves empty parts out', () => {
    const prompt = agentPrompt([
      finding({ suggested_action: 'Do it.', validation_expectation: 'Run it.' }),
      finding({ anchors: [], title: 'Other' }),
    ])
    expect(prompt).toContain('- In `src/a.ts:3-5`: Title. Why. Fix: Do it. Check: Run it.\n')
    expect(prompt.endsWith('- Other. Why.')).toBe(true)
  })
})

describe('formatSeconds and proseParts', () => {
  it('shows minutes only from a minute on and splits code spans', () => {
    expect(formatSeconds(95)).toBe('1 min 35 s')
    expect(formatSeconds(42)).toBe('42 s')
    expect(proseParts('Use `abs_diff` here')).toEqual([
      { text: 'Use ', code: false },
      { text: 'abs_diff', code: true },
      { text: ' here', code: false },
    ])
    expect(proseParts('a ` b')).toEqual([{ text: 'a ` b', code: false }])
  })
})

describe('highlightLines and parseUnifiedDiff', () => {
  it('tokenizes supported languages without HTML markup and parses side-by-side pairs', () => {
    const lines = highlightLines('pub fn add(a: i32) -> i32 {\n    a + 1\n}\n', 'src/lib.rs')
    expect(lines).toHaveLength(3)
    expect(lines[0]?.some((t) => t.kind === 'keyword' && t.text === 'pub')).toBe(true)

    const hunks = parseUnifiedDiff(
      'diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n fn a() {\n-    old()\n+    new()\n\\ No newline at end of file\n',
    )
    expect(hunks).toHaveLength(1)
    expect(hunks[0]?.pairs).toHaveLength(2)
    expect(hunks[0]?.pairs[1]?.left?.text).toBe('    old()')
    expect(hunks[0]?.pairs[1]?.right?.text).toBe('    new()')
    expect(hunks[0]?.pairs[1]?.right?.noNewline).toBe(true)
  })
})
