import { createRouter, createWebHistory } from 'vue-router'

export function createAppRouter() {
  return createRouter({
    history: createWebHistory(import.meta.env.BASE_URL),
    routes: [
      { path: '/', component: () => import('@/pages/index.vue'), meta: { title: 'Overview' } },
      {
        path: '/login',
        component: () => import('@/pages/login.vue'),
        meta: { public: true, title: 'Sign in' },
      },
      {
        path: '/:path(.*)*',
        component: () => import('@/pages/[...404].vue'),
        meta: { public: true, title: 'Not found' },
      },
    ],
  })
}
