/** Fewer outcomes than this say nothing about a share; only the count is shown. */
export const MIN_OUTCOMES = 5

/** `part` of `total` as a whole percent, or a dash below {@link MIN_OUTCOMES}. */
export function formatShare(part: number, total: number): string {
  return total < MIN_OUTCOMES ? '—' : `${Math.round((part / total) * 100)}%`
}
