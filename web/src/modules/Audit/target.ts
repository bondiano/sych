/** The page of an audit target such as `run:<id>`; `undefined` for one without a page. */
export function targetLink(target: string): string | undefined {
  const [kind, id] = target.split(':', 2)
  if (!id) return undefined
  if (kind === 'run') return `/runs/${id}`
  if (kind === 'finding') return `/findings/${id}`
  if (kind === 'waiver') return '/waivers'
  return undefined
}
