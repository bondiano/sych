import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, idempotencyKey, unwrap, type Schemas } from '@/modules/Api'

/** Rule suggestions of one repository, computed by the server from recent feedback. */
export function useSuggestions(id: Ref<number | undefined>) {
  return useQuery({
    queryKey: ['suggestions', id],
    enabled: computed(() => id.value !== undefined),
    queryFn: async () =>
      unwrap(
        await api.GET('/v1/repositories/{id}/suggestions', {
          params: { path: { id: id.value ?? 0 } },
        }),
      ),
  })
}

export interface Decision {
  repositoryId: number
  fingerprint: string
  body: Schemas['SuggestionDecision']
}

/** A maintainer sets a suggestion aside or records the pull request that applied it. */
export function useDecideSuggestion() {
  const queries = useQueryClient()
  return useMutation({
    // Recording the same decision again changes nothing, so every click carries its own key.
    mutationFn: async ({ repositoryId, fingerprint, body }: Decision) =>
      unwrap(
        await api.POST('/v1/repositories/{id}/suggestions/{fingerprint}/decision', {
          params: {
            path: { id: repositoryId, fingerprint },
            header: { 'Idempotency-Key': idempotencyKey() },
          },
          body,
        }),
      ),
    onSuccess: () => queries.invalidateQueries({ queryKey: ['suggestions'] }),
  })
}
