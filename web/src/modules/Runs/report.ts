import type { Schemas } from '@/modules/Api'

type Result = Schemas['ReviewResult']
type Finding = Schemas['Finding']
type CodeRef = Schemas['CodeRef']

/** What a link to the reviewed code needs; absent once retention deleted the source snapshot. */
export interface CodeLinks {
  repo: string
  head: string
  base: string
}

const SEVERITIES: Schemas['Severity'][] = ['CRITICAL', 'HIGH', 'MEDIUM', 'LOW', 'INFO']

const CATEGORIES: Record<Schemas['FindingCategory'], string> = {
  DEFECT: 'Bug',
  RULE_VIOLATION: 'Rule violation',
  IMPROVEMENT: 'Improvement',
  NIT: 'Nit',
}

export const categoryLabel = (category: Schemas['FindingCategory']) => CATEGORIES[category]

const isProblem = (f: Finding) => f.category === 'DEFECT' || f.category === 'RULE_VIOLATION'

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

const COVERAGE: Record<Schemas['Coverage'], string> = {
  COMPLETE_FOR_SCOPE: 'Coverage is complete for the reviewed scope.',
  PARTIAL: 'Coverage is partial: the gaps below were not checked, so this is not "no problems".',
  UNAVAILABLE: 'Coverage is unavailable: no substantive review was possible.',
}

/** The report's headline: a partial result never reads as "no problems". */
export function verdict(result: Result): Verdict {
  const found = problems(result)
  const high = found.filter((f) => f.severity === 'HIGH' || f.severity === 'CRITICAL').length
  const complete = result.coverage === 'COMPLETE_FOR_SCOPE'
  const noun = found.length === 1 ? 'problem' : 'problems'
  const title = !found.length
    ? complete
      ? 'None confirmed in the reviewed scope'
      : 'None confirmed in what could be checked'
    : high
      ? `${found.length} ${noun}, ${high} high or critical`
      : `${found.length} ${noun}`
  const tone = high ? 'bad' : found.length || !complete ? 'warn' : 'ok'
  return { tone, title, coverage: COVERAGE[result.coverage] }
}

/** `path:start-end`, the way the report names a place in the code. */
export function place(ref: CodeRef): string {
  return ref.start_line < ref.end_line
    ? `${ref.path}:${ref.start_line}-${ref.end_line}`
    : `${ref.path}:${ref.start_line}`
}

/** The hash of a `sha1:…` object id, for a GitHub URL. */
export const hex = (oid: string) => oid.slice(oid.indexOf(':') + 1)

/** The code on GitHub at the commit reviewed; the target tip's commit is not known here. */
export function codeLink(ref: CodeRef, links: CodeLinks | undefined): string | undefined {
  if (!links || ref.side === 'TARGET_TIP') {
    return undefined
  }

  const sha = ref.side === 'HEAD' ? links.head : links.base
  const lines =
    ref.start_line < ref.end_line ? `L${ref.start_line}-L${ref.end_line}` : `L${ref.start_line}`
  return `https://github.com/${links.repo}/blob/${sha}/${ref.path}#${lines}`
}

function task(f: Finding): string {
  const anchor = f.anchors[0]
  return [
    anchor ? `In \`${place(anchor)}\`: ${f.title}.` : `${f.title}.`,
    f.explanation,
    f.suggested_action && `Fix: ${f.suggested_action}`,
    f.validation_expectation && `Check: ${f.validation_expectation}`,
    ...(f.also ?? []).map((a) => `Also on these lines: ${a.title}. ${a.explanation}`),
  ]
    .filter(Boolean)
    .join(' ')
}

/** The instruction for a coding agent, as the report words it. */
export function agentPrompt(findings: Finding[]): string {
  const intro =
    'Verify each finding against the current code; fix only those that still apply and skip the ' +
    'rest with a short reason.'
  return `${intro}\n\n${findings.map((f) => `- ${task(f)}`).join('\n')}`
}

/** `95 s`, `12 min 4 s`. */
export function formatSeconds(seconds: number): string {
  const minutes = Math.floor(seconds / 60)
  return minutes ? `${minutes} min ${seconds % 60} s` : `${seconds} s`
}

/** Model prose split at its `code` spans; a lone backtick stays text. */
export function proseParts(text: string): { text: string; code: boolean }[] {
  return text
    .split(/`([^`\n]+)`/)
    .map((part, i) => ({ text: part, code: i % 2 === 1 }))
    .filter((part) => part.text)
}
