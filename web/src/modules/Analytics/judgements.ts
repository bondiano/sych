import { MIN_OUTCOMES } from '@/common/utilities/formatShare'
import type { Schemas } from '@/modules/Api'

export const DIMENSIONS = ['rule_id', 'category', 'severity', 'repository'] as const
export type Dimension = (typeof DIMENSIONS)[number]

export type Breakdown = {
  key: string
  total: number
  useful: number
  notUseful: number
  falsePositive: number
  wontFix: number
}

const FIELDS = {
  USEFUL: 'useful',
  NOT_USEFUL: 'notUseful',
  FALSE_POSITIVE: 'falsePositive',
  WONT_FIX: 'wontFix',
} as const

/** Judgements summed by one dimension, most judged first; a missing value is `unknown`. */
export function breakdown(rows: Schemas['Judged'][], by: Dimension): Breakdown[] {
  const groups = new Map<string, Breakdown>()
  for (const row of rows) {
    const key = row[by] ?? 'unknown'
    const group = groups.get(key) ?? {
      key,
      total: 0,
      useful: 0,
      notUseful: 0,
      falsePositive: 0,
      wontFix: 0,
    }
    const field = FIELDS[row.judgement as keyof typeof FIELDS]
    groups.set(key, {
      ...group,
      total: group.total + row.count,
      ...(field ? { [field]: group[field] + row.count } : {}),
    })
  }
  return [...groups.values()].sort((a, b) => b.total - a.total || a.key.localeCompare(b.key))
}

/** Rules people called false positives, judged often enough to tell; worst first. */
export function noisyRules(rows: Schemas['Judged'][]): Breakdown[] {
  return breakdown(rows, 'rule_id')
    .filter((r) => r.key !== 'unknown' && r.total >= MIN_OUTCOMES && r.falsePositive > 0)
    .sort((a, b) => b.falsePositive / b.total - a.falsePositive / a.total)
}
