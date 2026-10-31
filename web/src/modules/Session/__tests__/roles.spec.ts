import { describe, expect, it } from 'vitest'

import type { Schemas } from '@/modules/Api'

import { can } from '../roles'

const me: Schemas['Me'] = {
  actor: 'github:alice',
  kind: 'user',
  tenant_id: 'local',
  repositories: [
    { repository: { id: 1, connection_id: 'gh', full_name: 'o/r' }, role: 'triage' },
    { repository: { id: 2, connection_id: 'gh', full_name: 'o/s' }, role: 'reader' },
  ],
}

describe('can', () => {
  it('grants a role and every lower one on that repository only', () => {
    expect(can(me, 'triage', 1)).toBe(true)
    expect(can(me, 'reader', 1)).toBe(true)
    expect(can(me, 'writer', 1)).toBe(false)
    expect(can(me, 'triage', 2)).toBe(false)
  })

  it('grants nothing on an unknown repository or before the caller is known', () => {
    expect(can(me, 'reader', 3)).toBe(false)
    expect(can(undefined, 'reader', 1)).toBe(false)
  })
})
