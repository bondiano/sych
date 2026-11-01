import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

export interface JobFilter {
  repository_id?: number
  state?: string
}

export const JOB_STATES = ['QUEUED', 'CLAIMED', 'DONE', 'FAILED', 'CANCELLED'] as const

/** Jobs newest first with the run each serves, a page at a time; a writer's view. */
export function useJobs(filter: Ref<JobFilter>) {
  return useInfiniteQuery({
    queryKey: ['jobs', filter],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const query = { ...filter.value, before: pageParam }
      return unwrap(await api.GET('/v1/jobs', { params: { query } }))
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
  })
}

/** Publications still `PUBLISHING` or `UNCERTAIN`, oldest first. */
export function useUnresolved(repositoryId: Ref<number | undefined>) {
  return useQuery({
    queryKey: ['unresolved', repositoryId],
    queryFn: async () =>
      unwrap(
        await api.GET('/v1/publications', {
          params: { query: { repository_id: repositoryId.value } },
        }),
      ),
  })
}

export interface Reconcile {
  id: number
  reason: string
  /** One per submitted decision: a retry of it replays, never decides twice. */
  key: string
}

/** An operator's audited decision to create an `UNCERTAIN` publication again. */
export function useReconcile() {
  const queries = useQueryClient()
  return useMutation({
    mutationFn: async ({ id, reason, key }: Reconcile) =>
      unwrap(
        await api.POST('/v1/publications/{id}/reconcile', {
          params: { path: { id }, header: { 'Idempotency-Key': key } },
          body: { reason },
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}
