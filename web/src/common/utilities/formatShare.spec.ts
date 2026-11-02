import { describe, expect, it } from 'vitest'

import { formatShare } from './formatShare'

describe('formatShare', () => {
  it('rounds to a whole percent from five outcomes', () => {
    expect(formatShare(1, 5)).toBe('20%')
    expect(formatShare(2, 3 * 5)).toBe('13%')
    expect(formatShare(0, 8)).toBe('0%')
  })

  it('hides a share of too few outcomes', () => {
    expect(formatShare(1, 4)).toBe('—')
    expect(formatShare(0, 0)).toBe('—')
  })
})
