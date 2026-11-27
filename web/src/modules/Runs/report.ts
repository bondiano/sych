import type { Schemas } from '@/modules/Api'

import { strings, type Lang } from './i18n'

type Result = Schemas['ReviewResult']
type Finding = Schemas['Finding']
type ReportFinding = Schemas['ReportFinding']
type CodeRef = Schemas['CodeRef']
type ReportCodeLinks = Schemas['ReportCodeLinks']

/** What a link to the reviewed code needs; absent once retention deleted the source snapshot. */
export interface CodeLinks {
  repo: string
  head: string
  base: string
  provider?: 'github' | 'gitlab'
  baseUrl?: string
}

export type AnyCodeLinks = CodeLinks | ReportCodeLinks

const SEVERITIES: Schemas['Severity'][] = ['CRITICAL', 'HIGH', 'MEDIUM', 'LOW', 'INFO']

const CATEGORIES_EN: Record<Schemas['FindingCategory'], string> = {
  DEFECT: 'Bug',
  RULE_VIOLATION: 'Rule violation',
  IMPROVEMENT: 'Improvement',
  NIT: 'Nit',
}

const CATEGORIES_RU: Record<Schemas['FindingCategory'], string> = {
  DEFECT: 'Ошибка',
  RULE_VIOLATION: 'Нарушение правила',
  IMPROVEMENT: 'Улучшение',
  NIT: 'Мелочь',
}

export const categoryLabel = (category: Schemas['FindingCategory'], lang: Lang = 'en') =>
  (lang === 'ru' ? CATEGORIES_RU : CATEGORIES_EN)[category]

/** Replaces control and bidi override characters with visible `U+XXXX` tokens. */
export function safeText(text: string): string {
  let out = ''
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0
    const isCtrl =
      (code < 0x20 && ch !== '\n' && ch !== '\t') ||
      (code >= 0x7f && code <= 0x9f) ||
      code === 0x061c ||
      code === 0x200e ||
      code === 0x200f ||
      (code >= 0x2028 && code <= 0x202e) ||
      (code >= 0x2066 && code <= 0x2069)
    if (isCtrl) {
      out += `U+${code.toString(16).toUpperCase().padStart(4, '0')}`
    } else {
      out += ch
    }
  }
  return out
}

const isProblem = (f: { category: Schemas['FindingCategory'] }) =>
  f.category === 'DEFECT' || f.category === 'RULE_VIOLATION'

/** Bugs and rule violations, in the report's order: severity, then fingerprint. */
export function problems(result: Result): Finding[] {
  return result.findings
    .filter(isProblem)
    .sort(
      (a, b) =>
        SEVERITIES.indexOf(a.severity) - SEVERITIES.indexOf(b.severity) ||
        a.fingerprint.localeCompare(b.fingerprint),
    )
}

export function optional(result: Result): Finding[] {
  return result.findings.filter((f) => !isProblem(f))
}

export interface Verdict {
  tone: 'ok' | 'warn' | 'bad'
  title: string
  coverage: string
}

const COVERAGE_EN: Record<Schemas['Coverage'], string> = {
  COMPLETE_FOR_SCOPE: 'Coverage is complete for the reviewed scope.',
  PARTIAL: 'Coverage is partial: the gaps below were not checked, so this is not "no problems".',
  UNAVAILABLE: 'Coverage is unavailable: no substantive review was possible.',
}

const COVERAGE_RU: Record<Schemas['Coverage'], string> = {
  COMPLETE_FOR_SCOPE: 'Покрытие полное для проверенной области.',
  PARTIAL: 'Покрытие частичное: пробелы ниже не проверялись, поэтому это не «проблем нет».',
  UNAVAILABLE: 'Покрытие недоступно: содержательная проверка не состоялась.',
}

export function verdictFromFindings(
  findings: { category: Schemas['FindingCategory']; severity: Schemas['Severity'] }[],
  coverage: Schemas['Coverage'],
  lang: Lang = 'en',
): Verdict {
  const found = findings.filter(isProblem)
  const high = found.filter((f) => f.severity === 'HIGH' || f.severity === 'CRITICAL').length
  const complete = coverage === 'COMPLETE_FOR_SCOPE'
  const tone = high ? 'bad' : found.length || !complete ? 'warn' : 'ok'

  if (lang === 'ru') {
    const title = !found.length
      ? complete
        ? 'В проверенной области проблем не подтверждено'
        : 'В проверенной части проблем не подтверждено'
      : high
        ? `Проблем: ${found.length} (высоких или критических: ${high})`
        : `Проблем: ${found.length}`
    return { tone, title, coverage: COVERAGE_RU[coverage] }
  }

  const noun = found.length === 1 ? 'problem' : 'problems'
  const title = !found.length
    ? complete
      ? 'None confirmed in the reviewed scope'
      : 'None confirmed in what could be checked'
    : high
      ? `${found.length} ${noun}, ${high} high or critical`
      : `${found.length} ${noun}`
  return { tone, title, coverage: COVERAGE_EN[coverage] }
}

/** The report's headline: a partial result never reads as "no problems". */
export function verdict(result: Result, lang: Lang = 'en'): Verdict {
  return verdictFromFindings(result.findings, result.coverage, lang)
}

/** `path:start-end`, the way the report names a place in the code. */
export function place(ref: CodeRef): string {
  const cleanPath = safeText(ref.path)
  return ref.start_line < ref.end_line
    ? `${cleanPath}:${ref.start_line}-${ref.end_line}`
    : `${cleanPath}:${ref.start_line}`
}

/** The hash of a `sha1:…` object id, for a GitHub URL. */
export const hex = (oid: string) => oid.slice(oid.indexOf(':') + 1)

function encodeRepoPath(repoPath: string): string {
  return repoPath
    .split('/')
    .filter((segment) => segment.length > 0)
    .map((segment) => encodeURIComponent(segment))
    .join('/')
}

function safeHttpOrigin(raw: string): string | undefined {
  try {
    const parsed = new URL(raw)
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
      return undefined
    }

    return parsed.toString().replace(/\/+$/, '')
  } catch {
    // Hostile or malformed base URLs never become clickable links.
    return undefined
  }
}

/** The code on GitHub or GitLab at the commit reviewed; refuses non-HTTP schemes and target-tip refs. */
export function codeLink(ref: CodeRef, links: AnyCodeLinks | undefined): string | undefined {
  if (!links || ref.side === 'TARGET_TIP') {
    return undefined
  }

  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(ref.path)) {
    return undefined
  }

  const provider = links.provider ?? 'github'
  const rawBase = 'base_url' in links ? links.base_url : (links.baseUrl ?? 'https://github.com')
  const baseUrl = safeHttpOrigin(rawBase)
  if (!baseUrl) {
    return undefined
  }

  const repoPath = encodeRepoPath(links.repo)
  const filePath = encodeRepoPath(ref.path)
  if (!repoPath || !filePath) {
    return undefined
  }

  const sha = encodeURIComponent(ref.side === 'HEAD' ? links.head : links.base)
  if (provider === 'gitlab') {
    const lines =
      ref.start_line < ref.end_line ? `L${ref.start_line}-${ref.end_line}` : `L${ref.start_line}`
    return `${baseUrl}/${repoPath}/-/blob/${sha}/${filePath}#${lines}`
  }

  const lines =
    ref.start_line < ref.end_line ? `L${ref.start_line}-L${ref.end_line}` : `L${ref.start_line}`
  return `${baseUrl}/${repoPath}/blob/${sha}/${filePath}#${lines}`
}

type PromptFinding =
  | Pick<
      Finding,
      'anchors' | 'title' | 'explanation' | 'suggested_action' | 'validation_expectation' | 'also'
    >
  | Pick<
      ReportFinding,
      'anchors' | 'title' | 'explanation' | 'suggested_action' | 'validation_expectation' | 'also'
    >

function firstRef(f: PromptFinding): CodeRef | undefined {
  const first = f.anchors[0]
  if (!first) {
    return undefined
  }

  return 'reference' in first ? first.reference : first
}

function stripPicture(explanation: string): string {
  const idx = explanation.indexOf('\n\n```text\n')
  return idx >= 0 ? explanation.slice(0, idx) : explanation
}

function task(f: PromptFinding, lang: Lang): string {
  const anchor = firstRef(f)
  const inWord = lang === 'ru' ? 'В' : 'In'
  const fixWord = lang === 'ru' ? 'Исправить' : 'Fix'
  const checkWord = lang === 'ru' ? 'Проверить' : 'Check'
  const alsoWord = strings(lang).alsoOnLines
  const explanation = stripPicture(f.explanation)
  return [
    anchor ? `${inWord} \`${place(anchor)}\`: ${f.title}.` : `${f.title}.`,
    explanation,
    f.suggested_action && `${fixWord}: ${f.suggested_action}`,
    f.validation_expectation && `${checkWord}: ${f.validation_expectation}`,
    ...(f.also ?? []).map((a) => `${alsoWord}: ${a.title}. ${a.explanation}`),
  ]
    .filter(Boolean)
    .join(' ')
}

/** The instruction for a coding agent, as the report words it. */
export function agentPrompt(findings: PromptFinding[], lang: Lang = 'en'): string {
  const intro =
    lang === 'ru'
      ? 'Проверьте каждое замечание по текущему коду; исправьте только актуальные, остальные пропустите с короткой причиной.'
      : 'Verify each finding against the current code; fix only those that still apply and skip the rest with a short reason.'
  return `${intro}\n\n${findings.map((f) => `- ${task(f, lang)}`).join('\n')}`
}

/** `95 s`, `12 min 4 s`. */
export function formatSeconds(seconds: number): string {
  const minutes = Math.floor(seconds / 60)
  return minutes ? `${minutes} min ${seconds % 60} s` : `${seconds} s`
}

/** Model prose split at its `code` spans; a lone backtick stays text. */
export function proseParts(text: string): { text: string; code: boolean }[] {
  return safeText(text)
    .split(/`([^`\n]+)`/)
    .map((part, i) => ({ text: part, code: i % 2 === 1 }))
    .filter((part) => part.text)
}
