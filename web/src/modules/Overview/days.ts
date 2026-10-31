import type { Schemas } from '@/modules/Api'

const DAY_MS = 86_400_000

/** Every UTC day from `from` to `to`, with zeros where no model was called. */
export function everyDay(usage: Schemas['Usage']): Schemas['UsageDay'][] {
  const known = new Map(usage.days.map((d) => [d.day, d]))
  const out: Schemas['UsageDay'][] = []
  for (let t = Date.parse(usage.from); t <= Date.parse(usage.to); t += DAY_MS) {
    const day = new Date(t).toISOString().slice(0, 10)
    out.push(
      known.get(day) ?? {
        day,
        attempts: 0,
        cost_known_micro_usd: 0,
        unknown_cost_attempts: 0,
        input_tokens: 0,
        output_tokens: 0,
      },
    )
  }
  return out
}
