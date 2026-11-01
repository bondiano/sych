import { useInfiniteQuery, useQuery } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

/** The tenant's GitHub connections as the server runs them; an operator's view. */
export function useConnections() {
  return useQuery({
    queryKey: ['admin', 'connections'],
    queryFn: async () => unwrap(await api.GET('/v1/admin/connections')),
  })
}

export interface DeliveryFilter {
  connection_id?: string
  decision?: string
}

export const DECISIONS = [
  'SCHEDULED',
  'NO_CHANGE',
  'IGNORED',
  'BOT_COMMENT',
  'NO_REPOSITORY',
  'REPOSITORY_NOT_ALLOWED',
] as const

/** Webhook deliveries newest first, a page at a time; kept 30 days. */
export function useDeliveries(filter: Ref<DeliveryFilter>) {
  return useInfiniteQuery({
    queryKey: ['admin', 'deliveries', filter],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const query = { ...filter.value, before: pageParam }
      return unwrap(await api.GET('/v1/admin/deliveries', { params: { query } }))
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
  })
}

/** The effective server config with secrets named, never shown. */
export function useEffectiveConfig() {
  return useQuery({
    queryKey: ['admin', 'config'],
    queryFn: async () => unwrap(await api.GET('/v1/admin/config')),
  })
}
