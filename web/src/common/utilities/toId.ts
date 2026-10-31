/** A positive integer id from URL text; anything else is no id rather than `NaN`. */
export function toId(text: string | undefined): number | undefined {
  return text !== undefined && /^[1-9]\d*$/.test(text) ? Number(text) : undefined
}
