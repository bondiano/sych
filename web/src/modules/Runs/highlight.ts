import './prismSetup'

import Prism from 'prismjs'
import 'prismjs/components/prism-markup-templating'
import 'prismjs/components/prism-typescript'
import 'prismjs/components/prism-jsx'
import 'prismjs/components/prism-tsx'
import 'prismjs/components/prism-rust'
import 'prismjs/components/prism-go'
import 'prismjs/components/prism-python'
import 'prismjs/components/prism-sql'
import 'prismjs/components/prism-php'
import 'prismjs/components/prism-r'
import 'prismjs/components/prism-clojure'
import 'prismjs/components/prism-yaml'
import 'prismjs/components/prism-json'
import 'prismjs/components/prism-toml'
import 'prismjs/components/prism-bash'

export type TokenKind =
  | 'comment'
  | 'keyword'
  | 'string'
  | 'number'
  | 'function'
  | 'type'
  | 'operator'
  | 'punctuation'
  | 'attr'

export interface HighlightSpan {
  kind?: TokenKind
  text: string
}

/** Upper bounds keeping 5,000-line or minified files instantaneous in offline reports. */
export const MAX_HIGHLIGHT_CHARS = 80_000
export const MAX_LINE_CHARS = 1_000

const EXT_TO_GRAMMAR: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'jsx',
  vue: 'markup',
  html: 'markup',
  xml: 'markup',
  go: 'go',
  py: 'python',
  pyi: 'python',
  sql: 'sql',
  php: 'php',
  r: 'r',
  clj: 'clojure',
  cljs: 'clojure',
  cljc: 'clojure',
  edn: 'clojure',
  yml: 'yaml',
  yaml: 'yaml',
  json: 'json',
  toml: 'toml',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
}

export function grammarForPath(path: string | undefined): Prism.Grammar | undefined {
  if (!path) {
    return undefined
  }

  const base = path.slice(path.lastIndexOf('/') + 1)
  const dot = base.lastIndexOf('.')
  if (dot < 0) {
    return undefined
  }

  const ext = base.slice(dot + 1).toLowerCase()
  const key = EXT_TO_GRAMMAR[ext]
  return key ? Prism.languages[key] : undefined
}

function classify(types: string[]): TokenKind | undefined {
  for (const t of types) {
    switch (t) {
      case 'comment':
      case 'prolog':
      case 'doctype':
      case 'cdata':
        return 'comment'
      case 'keyword':
      case 'boolean':
      case 'important':
      case 'atrule':
        return 'keyword'
      case 'string':
      case 'char':
      case 'regex':
      case 'template-string':
      case 'attr-value':
        return 'string'
      case 'number':
      case 'constant':
      case 'symbol':
        return 'number'
      case 'function':
      case 'macro':
        return 'function'
      case 'class-name':
      case 'builtin':
      case 'namespace':
      case 'tag':
        return 'type'
      case 'attr-name':
      case 'property':
      case 'key':
        return 'attr'
      case 'operator':
        return 'operator'
      case 'punctuation':
        return 'punctuation'
      default:
        break
    }
  }
  return undefined
}

function pushText(lines: HighlightSpan[][], text: string, kind: TokenKind | undefined) {
  const parts = text.split('\n')
  for (let i = 0; i < parts.length; i++) {
    if (i > 0) {
      lines.push([])
    }

    const piece = parts[i] ?? ''
    if (!piece) {
      continue
    }

    const current = lines[lines.length - 1]
    if (!current) {
      continue
    }

    const prev = current[current.length - 1]
    if (prev && prev.kind === kind) {
      prev.text += piece
    } else {
      current.push(kind ? { kind, text: piece } : { text: piece })
    }
  }
}

function walkTokens(
  stream: string | Prism.Token | (string | Prism.Token)[],
  inherited: string[],
  lines: HighlightSpan[][],
) {
  if (typeof stream === 'string') {
    pushText(lines, stream, classify(inherited))
    return
  }

  if (Array.isArray(stream)) {
    for (const item of stream) {
      walkTokens(item, inherited, lines)
    }
    return
  }

  const aliases = Array.isArray(stream.alias) ? stream.alias : stream.alias ? [stream.alias] : []
  const nextTypes = [stream.type, ...aliases, ...inherited]
  walkTokens(stream.content, nextTypes, lines)
}

function plainLines(rawLines: string[]): HighlightSpan[][] {
  return rawLines.map((line) => (line ? [{ text: line }] : []))
}

/**
 * Splits `code` into highlighted token arrays per line. Never produces HTML strings: callers
 * render every `span.text` via Vue text interpolation.
 */
export function highlightLines(code: string, path?: string): HighlightSpan[][] {
  const normalized = code.endsWith('\n') ? code.slice(0, -1) : code
  if (!normalized) {
    return []
  }

  const rawLines = normalized.split('\n')
  const grammar = grammarForPath(path)
  if (
    !grammar ||
    normalized.length > MAX_HIGHLIGHT_CHARS ||
    rawLines.some((line) => line.length > MAX_LINE_CHARS)
  ) {
    return plainLines(rawLines)
  }

  const lines: HighlightSpan[][] = [[]]
  const tokens = Prism.tokenize(normalized, grammar)
  walkTokens(tokens, [], lines)
  return lines
}

/** Tokenizes a single line of code (such as inside a diff row). */
export function highlightSingleLine(line: string, path?: string): HighlightSpan[] {
  if (!line) {
    return []
  }

  const grammar = grammarForPath(path)
  if (!grammar || line.length > MAX_LINE_CHARS) {
    return [{ text: line }]
  }

  const lines: HighlightSpan[][] = [[]]
  walkTokens(Prism.tokenize(line, grammar), [], lines)
  return lines[0] ?? [{ text: line }]
}
