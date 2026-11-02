import { describe, expect, it } from 'vitest'

import type { Schemas } from '@/modules/Api'

import { breakdown, noisyRules } from '../judgements'

const row = (rule_id: string | null, judgement: string, count: number): Schemas['Judged'] => ({
  repository: 'o/r',
  rule_id,
  category: 'BUG',
  severity: null,
  judgement,
  count,
})

describe('breakdown', () => {
  it('sums judgements per value, a missing one as unknown', () => {
    const rows = [row('A', 'USEFUL', 2), row('A', 'FALSE_POSITIVE', 1), row(null, 'WONT_FIX', 4)]
    expect(breakdown(rows, 'rule_id')).toEqual([
      { key: 'unknown', total: 4, useful: 0, notUseful: 0, falsePositive: 0, wontFix: 4 },
      { key: 'A', total: 3, useful: 2, notUseful: 0, falsePositive: 1, wontFix: 0 },
    ])
    expect(breakdown(rows, 'severity').map((b) => [b.key, b.total])).toEqual([['unknown', 7]])
  })
})

describe('noisyRules', () => {
  it('ranks rules by false positive share once judged five times', () => {
    const rows = [
      row('A', 'FALSE_POSITIVE', 1),
      row('A', 'USEFUL', 4),
      row('B', 'FALSE_POSITIVE', 3),
      row('B', 'USEFUL', 2),
      row('C', 'FALSE_POSITIVE', 4),
      row(null, 'FALSE_POSITIVE', 9),
    ]
    expect(noisyRules(rows).map((r) => r.key)).toEqual(['B', 'A'])
  })
})
