import { describe, expect, it } from 'vitest'

import { counted } from './counted'

describe('counted', () => {
  it('uses the singular only for one', () => {
    expect(counted(1, 'call')).toBe('1 call')
    expect(counted(0, 'call')).toBe('0 calls')
    expect(counted(3, 'finding')).toBe('3 findings')
  })

  it('takes an irregular plural', () => {
    expect(counted(2, 'person', 'people')).toBe('2 people')
  })
})
