import { useQuery, type QueryClient } from '@tanstack/vue-query'
import type { NavigationGuard } from 'vue-router'

import { api, ApiError, unwrap } from '@/modules/Api'

const ME = ['me'] as const

async function fetchMe() {
  return unwrap(await api.GET('/v1/me'))
}

/** Who is signed in and their role on each repository they may read. */
export function useMe() {
  return useQuery({ queryKey: ME, queryFn: fetchMe, staleTime: 60_000 })
}

/** The connections a person signs in through; each `login_url` is a full-page navigation. */
export function useSignIns() {
  return useQuery({
    queryKey: ['sign-ins'],
    queryFn: async () => unwrap(await api.GET('/auth/github')),
  })
}

/** A browser session acting as the API client whose token this is. */
export async function signInWithToken(token: string): Promise<void> {
  unwrap(await api.POST('/auth/token', { headers: { Authorization: `Bearer ${token}` } }))
}

export async function signOut(queries: QueryClient): Promise<void> {
  unwrap(await api.POST('/auth/logout'))
  queries.clear()
}

/** Pages need a session; without one the browser signs in and comes back. */
export function sessionGuard(queries: QueryClient): NavigationGuard {
  return async (to) => {
    if (to.meta.public) {
      return true
    }

    try {
      await queries.ensureQueryData({ queryKey: ME, queryFn: fetchMe })
      return true
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        return { path: '/login', query: { next: to.fullPath } }
      }

      throw e
    }
  }
}
