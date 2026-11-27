export { useAttempts, useResult, useRun, useSnapshots } from './queries'
export { parseUnifiedDiff } from './diff'
export type { DiffHunk, DiffPair, DiffRow, DiffRowKind } from './diff'
export {
  grammarForPath,
  highlightLines,
  highlightSingleLine,
  MAX_HIGHLIGHT_CHARS,
  MAX_LINE_CHARS,
} from './highlight'
export type { HighlightSpan, TokenKind } from './highlight'
export { strings } from './i18n'
export type { Lang, ReportStrings } from './i18n'
export {
  agentPrompt,
  categoryLabel,
  codeLink,
  formatSeconds,
  hex,
  optional,
  place,
  problems,
  proseParts,
  safeText,
  verdict,
  verdictFromFindings,
} from './report'
export type { AnyCodeLinks, CodeLinks, Verdict } from './report'
export { default as AttemptsTable } from './ui/AttemptsTable.vue'
export { default as CandidateList } from './ui/CandidateList.vue'
export { default as CodeExcerpt } from './ui/CodeExcerpt.vue'
export { default as CodePlace } from './ui/CodePlace.vue'
export { default as CopyPrompt } from './ui/CopyPrompt.vue'
export { default as DiffView } from './ui/DiffView.vue'
export { default as FindingArticle } from './ui/FindingArticle.vue'
export { default as ProseText } from './ui/ProseText.vue'
export { default as ResultSummary } from './ui/ResultSummary.vue'
export { default as RunChecks } from './ui/RunChecks.vue'
export { default as RunDisclosure } from './ui/RunDisclosure.vue'
export { default as RunFacts } from './ui/RunFacts.vue'
export { default as RunScope } from './ui/RunScope.vue'
export { default as RunVerdict } from './ui/RunVerdict.vue'
