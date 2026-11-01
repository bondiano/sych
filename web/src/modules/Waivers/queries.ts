import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, idempotencyKey, unwrap, type Schemas } from '@/modules/Api'

export interface WaiverFilter {
  repository_id?: number
  /** `active`, `expired` or `revoked`. */
  state?: string
}

/** Waivers newest first; the server sends them all at once. */
export function useWaivers(filter: Ref<WaiverFilter>) {
  return useQuery({
    queryKey: ['waivers', filter],
    queryFn: async () => unwrap(await api.GET('/v1/waivers', { params: { query: filter.value } })),
  })
}

export interface NewWaiver {
  body: Schemas['WaiverRequest']
  /** One per submitted content: a retry of it replays, never records twice. */
  key: string
}

/** A maintainer's scoped waiver of a rule; it lowers the risk decision, never a status. */
export function useCreateWaiver() {
  const queries = useQueryClient()
  return useMutation({
    mutationFn: async ({ body, key }: NewWaiver) =>
      unwrap(
        await api.POST('/v1/waivers', { params: { header: { 'Idempotency-Key': key } }, body }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}

/** The next review no longer applies it; revoking a revoked one keeps its time. */
export function useRevokeWaiver() {
  const queries = useQueryClient()
  return useMutation({
    // Revoking twice changes nothing, so every click may carry its own key.
    mutationFn: async (id: number) =>
      unwrap(
        await api.POST('/v1/waivers/{id}/revoke', {
          params: { path: { id }, header: { 'Idempotency-Key': idempotencyKey() } },
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}
