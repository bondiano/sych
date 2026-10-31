import { useInfiniteQuery, useQuery } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

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
