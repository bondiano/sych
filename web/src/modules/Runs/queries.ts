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

/** `null` for a 409: the run has no result to read from, not committed or deleted by retention. */
async function orNothing<T>(read: () => Promise<T>): Promise<T | null> {
  try {
    return await read()
  } catch (e) {
    if (e instanceof ApiError && e.status === 409) {
      return null
    }

    throw e
  }
}

/** The run's immutable result; `null` when it has none to show. */
export function useResult(id: Ref<string>) {
  return useQuery({
    queryKey: ['result', id],
    // A change without a run yet has no result to ask for.
    enabled: computed(() => id.value !== ''),
    queryFn: async () =>
      orNothing(async () =>
        unwrap(await api.GET('/v1/runs/{id}/result', { params: { path: { id: id.value } } })),
      ),
  })
}

/** The commits the result is bound to; `null` when it has none to show. */
export function useSnapshots(id: Ref<string>) {
  return useQuery({
    queryKey: ['snapshots', id],
    queryFn: async () =>
      orNothing(async () =>
        unwrap(await api.GET('/v1/runs/{id}/snapshots', { params: { path: { id: id.value } } })),
      ),
  })
}

export function useAttempts(id: Ref<string>) {
  return useQuery({
    queryKey: ['attempts', id],
    queryFn: async () =>
      unwrap(await api.GET('/v1/runs/{id}/attempts', { params: { path: { id: id.value } } })),
  })
}
