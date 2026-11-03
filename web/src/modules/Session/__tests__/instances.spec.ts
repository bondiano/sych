import { describe, expect, it } from 'vitest'

import { currentInstance, dashboardOf } from '../instances'

const production = { name: 'production', url: 'https://sych.example', current: true }
const sandbox = { name: 'sandbox', url: 'https://sandbox.sych.example', current: false }

describe('instances', () => {
  it('finds the server that answered and links to the dashboard of another', () => {
    expect(currentInstance([sandbox, production])).toBe(production)
    expect(currentInstance([])).toBeUndefined()
    expect(dashboardOf(sandbox)).toBe('https://sandbox.sych.example/ui/')
  })
})
