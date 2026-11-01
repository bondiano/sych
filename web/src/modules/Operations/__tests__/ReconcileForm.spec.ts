import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'

const sent: { url: string; key: string | null; body: unknown }[] = []
const replies: Response[] = []

vi.stubGlobal(
  'fetch',
  vi.fn(async (request: Request) => {
    sent.push({
      url: request.url,
      key: request.headers.get('idempotency-key'),
      body: await request.json(),
    })
    return (
      replies.shift() ??
      new Response('{"publication_id":4,"delivery":"PENDING","relevance":"CURRENT"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    )
  }),
)

// The client captures `fetch` when it is created, so the form is imported after the stub.
const { default: ReconcileForm } = await import('../ui/ReconcileForm.vue')

describe('ReconcileForm', () => {
  it('needs a reason and resends a failed decision with its key', async () => {
    replies.push(
      new Response('{"error":{"code":"DEPENDENCY_UNAVAILABLE","message":"down"}}', {
        status: 503,
        headers: { 'content-type': 'application/json' },
      }),
    )
    const wrapper = mount(ReconcileForm, {
      props: { publicationId: 4 },
      global: { plugins: [[VueQueryPlugin, { queryClient: new QueryClient() }]] },
    })
    expect(wrapper.find('button[type=submit]').attributes('disabled')).toBeDefined()
    await wrapper.find('textarea').setValue(' no comment on the PR ')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('down')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(sent).toHaveLength(2)
    expect(sent[0]?.url).toMatch(/\/v1\/publications\/4\/reconcile$/)
    expect(sent[0]?.body).toEqual({ reason: 'no comment on the PR' })
    expect(sent[1]?.key).toBe(sent[0]?.key)
  })
})
