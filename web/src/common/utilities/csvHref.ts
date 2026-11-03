/** A same-origin link that downloads `path` as CSV with the filters that are set. */
export function csvHref(
  path: string,
  filters: Record<string, string | number | undefined>,
): string {
  const query = new URLSearchParams({ format: 'csv' })
  for (const [name, value] of Object.entries(filters)) {
    if (value !== undefined) query.set(name, String(value))
  }
  return `${path}?${query}`
}
