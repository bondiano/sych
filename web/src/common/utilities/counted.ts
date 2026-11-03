/** `n` with its noun in the right number: `1 call`, `2 calls`; `many` for irregular plurals. */
export function counted(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`
}
