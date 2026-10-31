import { describe, expect, it } from 'vitest'

import { ApiError, unwrap } from '../client'

describe('unwrap', () => {
  it('returns the data of a successful answer', () => {
    expect(
      unwrap({ data: { actor: 'writer' }, response: new Response(null, { status: 200 }) }),
    ).toEqual({
      actor: 'writer',
    })
  })

  it('throws the envelope of a refusal with its status and code', () => {
    const error = {
      error: { code: 'FORBIDDEN', message: 'triage is needed', correlation_id: 'c-1' },
    }
    const refused = () => unwrap({ error, response: new Response(null, { status: 403 }) })
    expect(refused).toThrowError(ApiError)
    expect(refused).toThrowError(
      expect.objectContaining({
        message: 'triage is needed',
        status: 403,
        code: 'FORBIDDEN',
        correlationId: 'c-1',
      }),
    )
  })
})
