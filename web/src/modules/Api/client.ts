import createClient from 'openapi-fetch'

import type { components, paths } from './schema.gen'

export type Schemas = components['schemas']

/** A refused request with the server's error envelope. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly correlationId?: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

/** Same origin as `sych serve`: the session cookie goes along, no token lives in the page. */
export const api = createClient<paths>({ credentials: 'same-origin' })

/** A key for one user action; a retry of that action sends the same one. */
export function idempotencyKey(): string {
  return crypto.randomUUID()
}

interface Envelope {
  error?: { code?: string; message?: string; correlation_id?: string }
}

/** The answer's data, or its refusal as an {@link ApiError}. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.response.ok && result.data !== undefined) {
    return result.data
  }

  const body = result.error as Envelope | undefined
  throw new ApiError(
    result.response.status,
    body?.error?.code ?? 'UNKNOWN',
    body?.error?.message ?? result.response.statusText,
    body?.error?.correlation_id,
  )
}
