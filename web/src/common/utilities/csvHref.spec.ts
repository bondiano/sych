import { describe, expect, it } from 'vitest'

import { csvHref } from './csvHref'

describe('csvHref', () => {
  it('asks for CSV with only the filters that are set, encoded', () => {
    expect(csvHref('/v1/findings', { repository_id: 3, state: undefined, rule_id: 'A&B' })).toBe(
      '/v1/findings?format=csv&repository_id=3&rule_id=A%26B',
    )
    expect(csvHref('/v1/audit', {})).toBe('/v1/audit?format=csv')
  })
})
