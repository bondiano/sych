import { useQuery } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap } from '@/modules/Api'

const DAY = 86_400

/** Judgements and settled findings of the last `days` days. */
export function useQuality(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['quality', repositoryId, days],
    queryFn: async () => {
      const since = Math.floor(Date.now() / 1000) - days.value * DAY
      const query = { repository_id: repositoryId.value, since }
      return unwrap(await api.GET('/v1/analytics/quality', { params: { query } }))
    },
  })
}

/** Why reviews of the last `days` days stopped short. */
export function useHonesty(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['honesty', repositoryId, days],
    queryFn: async () => {
      const since = Math.floor(Date.now() / 1000) - days.value * DAY
      const query = { repository_id: repositoryId.value, since }
      return unwrap(await api.GET('/v1/analytics/honesty', { params: { query } }))
    },
  })
}

/** Model cost of the last `days` days, and the tenant's budget use for a caller of all of it. */
export function useCost(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['cost', repositoryId, days],
    queryFn: async () => {
      const since = Math.floor(Date.now() / 1000) - days.value * DAY
      const query = { repository_id: repositoryId.value, since }
      return unwrap(await api.GET('/v1/analytics/cost', { params: { query } }))
    },
  })
}

/** Time to delivery, run duration and queue wait of the last `days` days. */
export function useLatency(repositoryId: Ref<number | undefined>, days: Ref<number>) {
  return useQuery({
    queryKey: ['latency', repositoryId, days],
    queryFn: async () => {
      const since = Math.floor(Date.now() / 1000) - days.value * DAY
      const query = { repository_id: repositoryId.value, since }
      return unwrap(await api.GET('/v1/analytics/latency', { params: { query } }))
    },
  })
}
