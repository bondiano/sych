const small = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  maximumFractionDigits: 4,
})
const large = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' })

/** Micro-USD as dollars; an unknown amount is said so, never shown as zero. */
export function formatMicroUsd(micro: number | null | undefined): string {
  if (micro === null || micro === undefined) {
    return 'unknown'
  }

  const dollars = micro / 1_000_000
  return (Math.abs(dollars) < 1 ? small : large).format(dollars)
}
