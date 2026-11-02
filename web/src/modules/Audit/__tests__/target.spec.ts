import { describe, expect, it } from 'vitest'

import { targetLink } from '../target'

describe('targetLink', () => {
  it('links the targets that have a page', () => {
    expect(targetLink('run:0f1e')).toBe('/runs/0f1e')
    expect(targetLink('finding:12')).toBe('/findings/12')
    expect(targetLink('waiver:3')).toBe('/waivers')
  })

  it('leaves the rest unlinked', () => {
    expect(targetLink('publication_operation:4')).toBeUndefined()
    expect(targetLink('session')).toBeUndefined()
    expect(targetLink('run:')).toBeUndefined()
  })
})
