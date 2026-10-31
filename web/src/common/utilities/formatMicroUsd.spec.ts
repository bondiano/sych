import { describe, expect, it } from 'vitest'

import { formatMicroUsd } from './formatMicroUsd'

describe('formatMicroUsd', () => {
  it('keeps cents of a cent for small amounts and cents for large ones', () => {
    expect(formatMicroUsd(3_283)).toBe('$0.0033')
    expect(formatMicroUsd(1_234_567)).toBe('$1.23')
    expect(formatMicroUsd(0)).toBe('$0.00')
  })

  it('says unknown instead of zero', () => {
    expect(formatMicroUsd(null)).toBe('unknown')
    expect(formatMicroUsd(undefined)).toBe('unknown')
  })
})
