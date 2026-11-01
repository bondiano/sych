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
      new Response('{"waiver_id":7}', {
        status: 201,
        headers: { 'content-type': 'application/json' },
      })
    )
  }),
)

// The client captures `fetch` when it is created, so the form is imported after the stub.
const { default: WaiverForm } = await import('../ui/WaiverForm.vue')

function form() {
  return mount(WaiverForm, {
    props: { changeId: 3, ruleId: 'no-secrets', path: 'src/a.ts' },
    global: { plugins: [[VueQueryPlugin, { queryClient: new QueryClient() }]] },
  })
}

afterEach(() => {
  sent.length = 0
  replies.length = 0
})

describe('WaiverForm', () => {
  it('sends the rule, its paths and an expiry as an instant', async () => {
    const wrapper = form()
    await wrapper.find('#waiver-paths').setValue('src/a.ts\n\n  src/b.ts ')
    await wrapper.find('#waiver-owner').setValue(' alice ')
    await wrapper.find('#waiver-reason').setValue('fixture key')
    await wrapper.find('#waiver-expires').setValue('2027-02-01T12:30')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(sent).toHaveLength(1)
    expect(sent[0]?.body).toEqual({
      change_id: 3,
      rule_id: 'no-secrets',
      paths: ['src/a.ts', 'src/b.ts'],
      owner: 'alice',
      reason: 'fixture key',
      expires_at: new Date('2027-02-01T12:30').toISOString(),
      until: null,
    })
    expect(wrapper.text()).toContain('Waiver 7 recorded')
  })

  it('needs a future expiry or a condition, and resends a failure with its key', async () => {
    replies.push(
      new Response('{"error":{"code":"DEPENDENCY_UNAVAILABLE","message":"down"}}', {
        status: 503,
        headers: { 'content-type': 'application/json' },
      }),
    )
    const wrapper = form()
    await wrapper.find('#waiver-owner').setValue('alice')
    await wrapper.find('#waiver-reason').setValue('until the rewrite')
    expect(wrapper.find('button[type=submit]').attributes('disabled')).toBeDefined()
    await wrapper.find('#waiver-expires').setValue('2020-01-01T00:00')
    expect(wrapper.find('button[type=submit]').attributes('disabled')).toBeDefined()
    await wrapper.find('#waiver-expires').setValue('')
    await wrapper.find('#waiver-until').setValue('the rewrite lands')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('down')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(sent[0]?.key).toBeTruthy()
    expect(sent.map((s) => s.key)).toEqual([sent[0]?.key, sent[0]?.key])
    expect(sent[0]?.body).toMatchObject({ paths: ['src/a.ts'], expires_at: null })
  })
})
