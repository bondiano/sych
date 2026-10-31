import { computed, type WritableComputedRef } from 'vue'
import { useRoute, useRouter } from 'vue-router'

/** A query parameter as state: it reads the URL and writes it back, so a view can be linked. */
export function useQueryParam(name: string): WritableComputedRef<string | undefined> {
  const route = useRoute()
  const router = useRouter()
  return computed({
    get: () => {
      const value = route.query[name]
      return typeof value === 'string' && value !== '' ? value : undefined
    },
    set: (value) => {
      void router.replace({ query: { ...route.query, [name]: value || undefined } })
    },
  })
}
