import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap, type Schemas } from '@/modules/Api'

export interface FindingFilter {
  repository_id?: number
  change_id?: number
  state?: string
  category?: string
  severity?: string
  rule_id?: string
}

/** Findings newest first, a page at a time. */
export function useFindings(filter: Ref<FindingFilter>) {
  return useInfiniteQuery({
    queryKey: ['findings', filter],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const query = { ...filter.value, before: pageParam }
      return unwrap(await api.GET('/v1/findings', { params: { query } }))
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
  })
}

export function useFinding(id: Ref<number>) {
  return useQuery({
    queryKey: ['finding', id],
    queryFn: async () =>
      unwrap(await api.GET('/v1/findings/{id}', { params: { path: { id: id.value } } })),
  })
}

export const JUDGEMENTS = ['USEFUL', 'NOT_USEFUL', 'FALSE_POSITIVE', 'WONT_FIX'] as const
export type Judgement = (typeof JUDGEMENTS)[number]

export interface Feedback {
  body: Schemas['FeedbackRequest']
  /** One per submitted content: a retry of it replays, never records twice. */
  key: string
}

/** Feedback on one finding; `FALSE_POSITIVE` and `WONT_FIX` dismiss it. */
export function useFeedback(id: Ref<number>) {
  const queries = useQueryClient()
  return useMutation({
    mutationFn: async ({ body, key }: Feedback) =>
      unwrap(
        await api.POST('/v1/findings/{id}/feedback', {
          params: { path: { id: id.value }, header: { 'Idempotency-Key': key } },
          body,
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ predicate: (q) => q.queryKey[0] !== 'me' }),
  })
}
