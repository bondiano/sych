const format = new Intl.DateTimeFormat('en-GB', { dateStyle: 'medium', timeStyle: 'short' })

/** Unix seconds as a local date and time; `—` when there is none yet. */
export function formatTime(seconds: number | null | undefined): string {
  return seconds === null || seconds === undefined ? '—' : format.format(seconds * 1000)
}

/** A duration in seconds as `45 s`, `12 min`, `3 h` or `2 d`. */
export function formatAge(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)} s`
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`
  if (seconds < 86_400) return `${Math.round(seconds / 3600)} h`
  return `${Math.round(seconds / 86_400)} d`
}
