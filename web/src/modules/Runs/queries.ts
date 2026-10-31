import { useQuery } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, ApiError, unwrap } from '@/modules/Api'

export function useRun(id: Ref<string>) {
  return useQuery({
    queryKey: ['run', id],
    queryFn: async () =>
      unwrap(await api.GET('/v1/runs/{id}', { params: { path: { id: id.value } } })),
  })
}

/**
 * The run's immutable result; `null` when it has none to show: not committed (`NO_RESULT`) or
 * deleted by retention (`RESULT_PRUNED`).
 */
export function useResult(id: Ref<string>) {
  return useQuery({
    queryKey: ['result', id],
    // A change without a run yet has no result to ask for.
    enabled: computed(() => id.value !== ''),
    queryFn: async () => {
      try {
        return unwrap(await api.GET('/v1/runs/{id}/result', { params: { path: { id: id.value } } }))
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) {
          return null
        }

        throw e
      }
    },
  })
}

export function useAttempts(id: Ref<string>) {
  return useQuery({
    queryKey: ['attempts', id],
    queryFn: async () =>
      unwrap(await api.GET('/v1/runs/{id}/attempts', { params: { path: { id: id.value } } })),
  })
}
