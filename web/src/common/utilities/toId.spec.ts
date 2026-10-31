import { describe, expect, it } from 'vitest'

import { toId } from './toId'

describe('toId', () => {
  it('reads a positive integer and nothing else', () => {
    expect(toId('42')).toBe(42)
    expect(toId(undefined)).toBeUndefined()
    for (const bad of ['abc', '0', '-1', '1.5', '']) {
      expect(toId(bad)).toBeUndefined()
    }
  })
})
