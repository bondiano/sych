import { describe, expect, it } from 'vitest'

import type { Schemas } from '@/modules/Api'

import { EXPIRING_SECONDS, waiverState } from '../state'

const NOW = 1_800_000_000

function waiver(patch: Partial<Schemas['WaiverEntry']>): Schemas['WaiverEntry'] {
  return {
    id: 1,
    repository: { id: 1, connection_id: 'gh', full_name: 'o/r' },
    rule_id: 'no-secrets',
    paths: ['src/a.ts'],
    reason: 'fixture',
    owner: 'alice',
    author: 'github:alice',
    created_at: NOW - 100,
    ...patch,
  }
}

describe('waiverState', () => {
  it('counts expiry as the server does: at `expires_at` it is over', () => {
    expect(waiverState(waiver({ expires_at: NOW }), NOW)).toBe('EXPIRED')
    expect(waiverState(waiver({ expires_at: NOW + 1 }), NOW)).toBe('EXPIRING')
    expect(waiverState(waiver({ expires_at: NOW + EXPIRING_SECONDS }), NOW)).toBe('EXPIRING')
    expect(waiverState(waiver({ expires_at: NOW + EXPIRING_SECONDS + 1 }), NOW)).toBe('ACTIVE')
  })

  it('keeps a revoked one revoked and a condition-only one active', () => {
    expect(waiverState(waiver({ expires_at: NOW - 1, revoked_at: NOW - 5 }), NOW)).toBe('REVOKED')
    expect(waiverState(waiver({ until: 'the migration lands' }), NOW)).toBe('ACTIVE')
  })
})
