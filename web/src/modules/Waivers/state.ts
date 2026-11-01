import type { Schemas } from '@/modules/Api'

export type WaiverState = 'ACTIVE' | 'EXPIRING' | 'EXPIRED' | 'REVOKED'

/** How close to its expiry a waiver is shown as expiring. */
export const EXPIRING_SECONDS = 7 * 86_400

/**
 * A waiver's state at `now` (Unix seconds), as the server counts it: revoked first, then expired
 * once `expires_at` passed. One with only an `until` condition never expires by itself.
 */
export function waiverState(waiver: Schemas['WaiverEntry'], now: number): WaiverState {
  if (waiver.revoked_at != null) return 'REVOKED'
  if (waiver.expires_at == null) return 'ACTIVE'
  if (waiver.expires_at <= now) return 'EXPIRED'
  return waiver.expires_at - now <= EXPIRING_SECONDS ? 'EXPIRING' : 'ACTIVE'
}
