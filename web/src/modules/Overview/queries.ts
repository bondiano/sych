import { useQuery } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

const DAY = 86_400

function isoDay(seconds: number): string {
  return new Date(seconds * 1000).toISOString().slice(0, 10)
}

/** Totals of the last `days` days, over one repository or every readable one. */
export function useSummary(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['summary', repositoryId, days],
    queryFn: async () => {
      const since = Math.floor(Date.now() / 1000) - days.value * DAY
      const query = { repository_id: repositoryId.value, since }
      return unwrap(await api.GET('/v1/summary', { params: { query } }))
    },
  })
}

/** Model usage per UTC day over the same period. */
export function useUsage(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['usage', repositoryId, days],
    queryFn: async () => {
      const now = Math.floor(Date.now() / 1000)
      const query = {
        repository_id: repositoryId.value,
        from: isoDay(now - (days.value - 1) * DAY),
        to: isoDay(now),
      }
      return unwrap(await api.GET('/v1/usage', { params: { query } }))
    },
  })
}
