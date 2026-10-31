const format = new Intl.DateTimeFormat('en-GB', { dateStyle: 'medium', timeStyle: 'short' })

/** Unix seconds as a local date and time; `—` when there is none yet. */
export function formatTime(seconds: number | null | undefined): string {
  return seconds === null || seconds === undefined ? '—' : format.format(seconds * 1000)
}
