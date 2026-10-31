import { createRouter, createWebHistory } from 'vue-router'

export function createAppRouter() {
  return createRouter({
    history: createWebHistory(import.meta.env.BASE_URL),
    routes: [
      { path: '/', component: () => import('@/pages/index.vue'), meta: { title: 'Overview' } },
      {
        path: '/changes',
        component: () => import('@/pages/changes/index.vue'),
        meta: { title: 'Pull requests' },
      },
      {
        path: '/changes/:id(\\d+)',
        component: () => import('@/pages/changes/[id].vue'),
        meta: { title: 'Pull request' },
      },
      {
        path: '/findings',
        component: () => import('@/pages/findings/index.vue'),
        meta: { title: 'Findings' },
      },
      {
        path: '/findings/:id(\\d+)',
        component: () => import('@/pages/findings/[id].vue'),
        meta: { title: 'Finding' },
      },
      {
        path: '/runs/:id',
        component: () => import('@/pages/runs/[id].vue'),
        meta: { title: 'Run' },
      },
      {
        path: '/runs/:id/tour',
        component: () => import('@/pages/runs/[id]/tour.vue'),
        meta: { title: 'Tour' },
      },
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
