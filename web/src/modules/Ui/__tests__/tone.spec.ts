import { describe, expect, it } from 'vitest'

import { label, tone } from '../lib/tone'

describe('tone', () => {
  it('shows an unknown or partial outcome as needing a look, never as fine', () => {
    expect(tone('UNKNOWN')).toBe('warn')
    expect(tone('PARTIAL')).toBe('warn')
    expect(tone('COMPLETE_FOR_SCOPE')).toBe('ok')
    expect(tone('ATTENTION_REQUIRED')).toBe('bad')
  })

  it('leaves an unmapped value neutral and nothing unlabelled', () => {
    expect(tone('SOMETHING_NEW')).toBeUndefined()
    expect(tone('constructor')).toBeUndefined()
    expect(tone(null)).toBeUndefined()
    expect(label('NEEDS_EVIDENCE')).toBe('needs evidence')
    expect(label(undefined)).toBe('—')
  })
})
