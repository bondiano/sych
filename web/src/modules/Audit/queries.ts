import { useInfiniteQuery } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

export interface AuditFilter {
  action?: string
}

/** Recorded actions on the repositories the caller maintains, newest first, a page at a time. */
export function useAudit(filter: Ref<AuditFilter>) {
  return useInfiniteQuery({
    queryKey: ['audit', filter],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const query = { ...filter.value, before: pageParam }
      return unwrap(await api.GET('/v1/audit', { params: { query } }))
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
  })
}

/** The actions the server records; the filter offers these. */
export const ACTIONS = [
  'RUN_REQUESTED',
  'RUN_CANCELLED',
  'CI_JOB_CANCELLED',
  'FINDING_FEEDBACK',
  'WAIVER_CREATED',
  'WAIVER_REVOKED',
  'PUBLICATION_REQUESTED',
  'PUBLICATION_RECREATE_AUTHORIZED',
  'PUBLICATION_RECREATE_AUTOMATIC',
  'SESSION_STARTED',
  'SESSION_ENDED',
] as const
