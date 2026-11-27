export type DiffRowKind = 'context' | 'del' | 'add'

export interface DiffRow {
  kind: DiffRowKind
  oldLine?: number
  newLine?: number
  text: string
  noNewline?: boolean
}

export interface DiffPair {
  left?: DiffRow
  right?: DiffRow
}

export interface DiffHunk {
  header: string
  rows: DiffRow[]
  pairs: DiffPair[]
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/
const NO_NEWLINE = '\\ No newline at end of file'

function pairRows(rows: DiffRow[]): DiffPair[] {
  const pairs: DiffPair[] = []
  let i = 0
  while (i < rows.length) {
    const row = rows[i]
    if (!row) {
      i++
      continue
    }

    if (row.kind === 'context') {
      pairs.push({ left: row, right: row })
      i++
      continue
    }

    const dels: DiffRow[] = []
    while (i < rows.length && rows[i]?.kind === 'del') {
      const del = rows[i]
      if (del) {
        dels.push(del)
      }
      i++
    }
    const adds: DiffRow[] = []
    while (i < rows.length && rows[i]?.kind === 'add') {
      const add = rows[i]
      if (add) {
        adds.push(add)
      }
      i++
    }
    const count = Math.max(dels.length, adds.length)
    for (let k = 0; k < count; k++) {
      pairs.push({ left: dels[k], right: adds[k] })
    }
  }
  return pairs
}

/** Parses a Git unified diff into structured hunks with both unified rows and side-by-side pairs. */
export function parseUnifiedDiff(diff: string): DiffHunk[] {
  const normalized = diff.replace(/\r\n/g, '\n')
  const rawLines = normalized.endsWith('\n')
    ? normalized.slice(0, -1).split('\n')
    : normalized.split('\n')
  const hunks: DiffHunk[] = []
  let current: DiffHunk | undefined
  let oldLine = 0
  let newLine = 0

  for (const line of rawLines) {
    if (
      !current &&
      (line.startsWith('diff --git ') || line.startsWith('--- ') || line.startsWith('+++ '))
    ) {
      continue
    }

    const match = HUNK_RE.exec(line)
    if (match) {
      if (current) {
        current.pairs = pairRows(current.rows)
        hunks.push(current)
      }
      oldLine = Number(match[1] ?? 0)
      newLine = Number(match[2] ?? 0)
      current = { header: line, rows: [], pairs: [] }
      continue
    }

    if (!current) {
      continue
    }

    if (line === NO_NEWLINE) {
      const last = current.rows[current.rows.length - 1]
      if (last) {
        last.noNewline = true
      }
      continue
    }

    const prefix = line[0]
    const text = line.slice(1)
    if (prefix === '-') {
      current.rows.push({ kind: 'del', oldLine, text })
      oldLine++
    } else if (prefix === '+') {
      current.rows.push({ kind: 'add', newLine, text })
      newLine++
    } else if (prefix === ' ') {
      current.rows.push({ kind: 'context', oldLine, newLine, text })
      oldLine++
      newLine++
    }
  }

  if (current) {
    current.pairs = pairRows(current.rows)
    hunks.push(current)
  }

  return hunks
}
