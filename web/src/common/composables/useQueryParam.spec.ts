import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'

import { useQueryParam } from './useQueryParam'

describe('useQueryParam', () => {
  it('keeps a filter in the URL and drops it when cleared', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', component: { render: () => null } }],
    })
    await router.push('/?repository=4')
    let state: ReturnType<typeof useQueryParam> | undefined
    mount(
      defineComponent({
        setup() {
          state = useQueryParam('state')
          return () => h('div')
        },
      }),
      { global: { plugins: [router] } },
    )
    expect(state?.value).toBeUndefined()
    state!.value = 'OPEN'
    await router.isReady()
    await new Promise((r) => setTimeout(r))
    expect(router.currentRoute.value.query).toEqual({ repository: '4', state: 'OPEN' })
    state!.value = undefined
    await new Promise((r) => setTimeout(r))
    expect(router.currentRoute.value.query).toEqual({ repository: '4' })
  })
})
