import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import type { Ref } from 'vue'

import { api, unwrap, type Schemas } from '@/modules/Api'

/** The 50 newest playground runs of readable repositories. */
export function usePlaygroundRuns(repositoryId: Ref<number | undefined>) {
  return useQuery({
    queryKey: ['playground', repositoryId],
    queryFn: async () =>
      unwrap(
        await api.GET('/v1/playground', {
          params: { query: { repository_id: repositoryId.value } },
        }),
      ),
  })
}

export interface Try {
  body: Schemas['PlaygroundRequest']
  /** One per chosen options: a retry replays, never starts a second try. */
  key: string
}

/** A try of the pull request's current state that publishes nothing. */
export function useTry(changeId: Ref<number>) {
  const queries = useQueryClient()
  return useMutation({
    mutationFn: async ({ body, key }: Try) =>
      unwrap(
        await api.POST('/v1/changes/{id}/playground', {
          params: { path: { id: changeId.value }, header: { 'Idempotency-Key': key } },
          body,
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ queryKey: ['playground'] }),
  })
}
