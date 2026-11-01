export type Tone = 'ok' | 'warn' | 'bad' | 'off'

// By meaning: settled well, needs a look, blocking, no longer in play. Anything else is neutral,
// never shown as fine.
const TONES = new Map<string, Tone>([
  ['COMPLETED', 'ok'],
  ['COMPLETE_FOR_SCOPE', 'ok'],
  ['NO_BLOCKING_FINDINGS_IN_SCOPE', 'ok'],
  ['RESOLVED', 'ok'],
  ['SUPPORTED', 'ok'],
  ['DELIVERED', 'ok'],
  ['SETTLED', 'ok'],
  ['PARTIAL', 'warn'],
  ['NEEDS_EVIDENCE', 'warn'],
  ['UNKNOWN', 'warn'],
  ['OPEN', 'warn'],
  ['MEDIUM', 'warn'],
  ['UNCERTAIN', 'warn'],
  ['RESERVED', 'warn'],
  ['WAITING_EVIDENCE', 'warn'],
  ['EXPIRING', 'warn'],
  ['ATTENTION_REQUIRED', 'bad'],
  ['FAILED', 'bad'],
  ['VIOLATED', 'bad'],
  ['HIGH', 'bad'],
  ['CRITICAL', 'bad'],
  ['UNAVAILABLE', 'bad'],
  ['CANCELLED', 'off'],
  ['SUPERSEDED', 'off'],
  ['DISMISSED', 'off'],
  ['OBSOLETE', 'off'],
  ['NOT_APPLICABLE', 'off'],
  ['RELEASED', 'off'],
  ['STALE', 'off'],
  ['EXPIRED', 'off'],
  ['REVOKED', 'off'],
])

export function tone(value: string | null | undefined): Tone | undefined {
  return value ? TONES.get(value) : undefined
}

/** `ATTENTION_REQUIRED` as `attention required`; nothing as `—`. */
export function label(value: string | null | undefined): string {
  return value ? value.toLowerCase().replace(/_/g, ' ') : '—'
}
