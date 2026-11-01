import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, idempotencyKey, unwrap, type Schemas } from '@/modules/Api'

export interface ChangeFilter {
  repository_id?: number
  state?: string
}

/** Pull requests newest first, a page at a time. */
export function useChanges(filter: Ref<ChangeFilter>) {
  return useInfiniteQuery({
    queryKey: ['changes', filter],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const query = { ...filter.value, before: pageParam }
      return unwrap(await api.GET('/v1/changes', { params: { query } }))
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
  })
}

export function useChange(id: Ref<number>) {
  return useQuery({
    queryKey: ['change', id],
    queryFn: async () =>
      unwrap(await api.GET('/v1/changes/{id}', { params: { path: { id: id.value } } })),
  })
}

/** The run states a cancel still stops; a committed run keeps its own. */
export const ACTIVE_STATES = ['QUEUED', 'WAITING_EVIDENCE', 'RUNNING']

/** Stops an active run before it commits. */
export function useCancelRun() {
  const queries = useQueryClient()
  return useMutation({
    // Cancelling twice changes nothing, so every click may carry its own key.
    mutationFn: async (id: string) =>
      unwrap(
        await api.POST('/v1/runs/{id}/cancel', {
          params: { path: { id }, header: { 'Idempotency-Key': idempotencyKey() } },
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}

export interface Rerun {
  body: Schemas['RerunRequest']
  /** One per chosen options: a retry replays, never starts a second generation. */
  key: string
}

/** A new generation from the pull request's current state. */
export function useRerun(id: Ref<number>) {
  const queries = useQueryClient()
  return useMutation({
    mutationFn: async ({ body, key }: Rerun) =>
      unwrap(
        await api.POST('/v1/changes/{id}/reruns', {
          params: { path: { id: id.value }, header: { 'Idempotency-Key': key } },
          body,
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}
