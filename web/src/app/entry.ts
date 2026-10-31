import './styles/main.css'

import { QueryCache, QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { createApp } from 'vue'

import { ApiError } from '@/modules/Api'
import { sessionGuard } from '@/modules/Session'

import App from './App.vue'
import { createAppRouter } from './router'

const router = createAppRouter()
const signedOut = (e: unknown) => e instanceof ApiError && e.status === 401
const queries = new QueryClient({
  // A session that ends while a page is open sends the browser to sign in again.
  queryCache: new QueryCache({
    onError: (e) => {
      if (signedOut(e)) {
        void router.push({ path: '/login', query: { next: router.currentRoute.value.fullPath } })
      }
    },
  }),
  defaultOptions: {
    // A refusal stays a refusal; only a server or network failure is tried again.
    queries: { retry: (count, e) => !(e instanceof ApiError && e.status < 500) && count < 2 },
  },
})

router.beforeEach(sessionGuard(queries))
router.afterEach((to) => {
  document.title = to.meta.title ? `${to.meta.title} · Sych` : 'Sych'
})

createApp(App).use(router).use(VueQueryPlugin, { queryClient: queries }).mount('#app')
