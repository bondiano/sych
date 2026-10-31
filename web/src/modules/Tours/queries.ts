import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, ApiError, idempotencyKey, unwrap } from '@/modules/Api'

/** The run's newest tour as the caller's own session sees it; `null` when there is none. */
export function useTour(runId: Ref<string>) {
  return useQuery({
    queryKey: ['tour', runId],
    queryFn: async () => {
      try {
        return unwrap(
          await api.GET('/v1/runs/{id}/tour', { params: { path: { id: runId.value } } }),
        )
      } catch (e) {
        if (e instanceof ApiError && (e.status === 404 || e.status === 409)) {
          return null
        }

        throw e
      }
    },
  })
}

/**
 * The caller's moves through a tour. Each is its own action with its own key; none of them is
 * an approval or changes the review.
 */
export function useTourActions(runId: Ref<string>, tourId: Ref<string | undefined>) {
  const queries = useQueryClient()
  const path = () => ({ id: runId.value, tour: tourId.value ?? '' })
  const header = () => ({ 'Idempotency-Key': idempotencyKey() })
  const done = () => queries.invalidateQueries({ queryKey: ['tour', runId] })
  const reveal = useMutation({
    mutationFn: async (stepId: string) =>
      unwrap(
        await api.POST('/v1/runs/{id}/tours/{tour}/reveal', {
          params: { path: path(), header: header() },
          body: { step_id: stepId },
        }),
      ),
    onSuccess: done,
  })
  const finish = useMutation({
    mutationFn: async (how: 'complete' | 'dismiss') =>
      unwrap(
        how === 'complete'
          ? await api.POST('/v1/runs/{id}/tours/{tour}/complete', {
              params: { path: path(), header: header() },
            })
          : await api.POST('/v1/runs/{id}/tours/{tour}/dismiss', {
              params: { path: path(), header: header() },
            }),
      ),
    onSuccess: done,
  })
  const ask = useMutation({
    mutationFn: async ({ stepId, question }: { stepId: string; question: string }) =>
      unwrap(
        await api.POST('/v1/runs/{id}/tours/{tour}/questions', {
          params: { path: path(), header: header() },
          body: { step_id: stepId, kind: 'COMPREHENSION', question, waiver: null },
        }),
      ),
    onSuccess: done,
  })
  return { reveal, finish, ask }
}
