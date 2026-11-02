import { useQuery } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

/** The policy of the repository's newest run, read again at its pinned source. */
export function usePolicy(id: Ref<number | undefined>) {
  return useQuery({
    queryKey: ['policy', id],
    enabled: computed(() => id.value !== undefined),
    queryFn: async () =>
      unwrap(
        await api.GET('/v1/repositories/{id}/policy', { params: { path: { id: id.value ?? 0 } } }),
      ),
  })
}
