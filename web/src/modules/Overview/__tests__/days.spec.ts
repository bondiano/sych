import { describe, expect, it } from 'vitest'

import { everyDay } from '../days'

describe('everyDay', () => {
  it('fills the days without calls so one busy day is one bar of many', () => {
    const busy = {
      day: '2026-10-30',
      attempts: 2,
      cost_known_micro_usd: 1200,
      unknown_cost_attempts: 1,
      input_tokens: 10,
      output_tokens: 5,
    }
    const days = everyDay({
      from: '2026-10-29',
      to: '2026-10-31',
      days: [busy],
      subscription_fees: [],
    })
    expect(days.map((d) => [d.day, d.cost_known_micro_usd])).toEqual([
      ['2026-10-29', 0],
      ['2026-10-30', 1200],
      ['2026-10-31', 0],
    ])
    expect(days[1]).toBe(busy)
  })
})
