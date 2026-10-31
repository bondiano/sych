import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'

const sent: { key: string | null; body: unknown }[] = []
const replies: Response[] = []

vi.stubGlobal(
  'fetch',
  vi.fn(async (request: Request) => {
    sent.push({ key: request.headers.get('idempotency-key'), body: await request.json() })
    return (
      replies.shift() ??
      new Response('{"finding_id":1}', {
        status: 201,
        headers: { 'content-type': 'application/json' },
      })
    )
  }),
)

// The client captures `fetch` when it is created, so the form is imported after the stub.
const { default: FeedbackForm } = await import('../ui/FeedbackForm.vue')

function form() {
  return mount(FeedbackForm, {
    props: { findingId: 1 },
    global: { plugins: [[VueQueryPlugin, { queryClient: new QueryClient() }]] },
  })
}

afterEach(() => {
  sent.length = 0
})

describe('FeedbackForm', () => {
  it('sends the judgement, its scope and the rationale', async () => {
    const wrapper = form()
    await wrapper.find('textarea').setValue(' guarded upstream ')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(sent).toHaveLength(1)
    expect(sent[0]?.body).toEqual({
      judgement: 'USEFUL',
      scope: 'finding',
      rationale: 'guarded upstream',
    })
    expect(sent[0]?.key).toMatch(/^[0-9a-f-]{36}$/)
  })

  it('resends a failed submission with its key, and new content with a new one', async () => {
    replies.push(
      new Response('{"error":{"code":"DEPENDENCY_UNAVAILABLE","message":"down"}}', {
        status: 503,
        headers: { 'content-type': 'application/json' },
      }),
    )
    const wrapper = form()
    await wrapper.find('textarea').setValue('first')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('down')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    await wrapper.find('textarea').setValue('second')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(sent.map((s) => s.key)).toEqual([sent[0]?.key, sent[0]?.key, expect.any(String)])
    expect(sent[2]?.key).not.toBe(sent[0]?.key)
  })
})
