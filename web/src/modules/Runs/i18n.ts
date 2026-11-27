import type { Schemas } from '@/modules/Api'

export type Lang = Schemas['ReviewLanguage']

export interface ReportStrings {
  reportTitle: string
  revision: string
  target: string
  diffBase: string
  head: string
  branch: string
  stagedHead: string
  worktreeHead: string
  snapshotExpired: string
  costKnown: string
  unknownCostCalls: string
  modelCalls: string
  tokensInOut: string
  engine: string
  limitReached: string
  fixFirst: string
  findingsByFile: string
  noFileAnchor: string
  noFindings: string
  noFilteredFindings: string
  filterSeverity: string
  filterCategory: string
  filterFilePlaceholder: string
  allSeverities: string
  allCategories: string
  clearFilters: string
  copyPromptAll: string
  copyPromptOne: string
  copyPatchAll: string
  copyPatchOne: string
  copied: string
  copyRefused: string
  sourceUnavailable: string
  openOnGithub: string
  openOnGitlab: string
  changeDiff: string
  suggestedFix: string
  deleteLines: string
  when: string
  consequence: string
  alsoOnLines: string
  suggested: string
  remediation: string
  toCheck: string
  limitation: string
  noEofNewline: string
  gapsHeading: string
  noGaps: string
  unknownObligations: string
  requiredTag: string
  unreviewedPaths: string
  hiddenInChange: string
  hiddenInChangeNote: string
  openQuestions: string
  unresolvedCandidates: string
  architectureHeading: string
  baseChecked: string
  baseNotChecked: string
  fixedInHead: string
  policyHeading: string
  policySource: string
  policyProposed: string
  obligationsHeading: string
  evidenceHeading: string
  scopeHeading: string
  advisoryNote: string
  sideOld: string
  sideNew: string
  expandLines: (count: number) => string
  collapseLines: string
}

const EN: ReportStrings = {
  reportTitle: 'Sych review',
  revision: 'revision',
  target: 'Target',
  diffBase: 'Diff base',
  head: 'Head',
  branch: 'Branch',
  stagedHead: 'staged index (uncommitted)',
  worktreeHead: 'working tree (uncommitted)',
  snapshotExpired: 'Source snapshot expired; findings remain from the saved result.',
  costKnown: 'known',
  unknownCostCalls: 'of unknown cost',
  modelCalls: 'Model calls',
  tokensInOut: 'Tokens in / out',
  engine: 'Engine',
  limitReached: 'A budget or size limit stopped part of the review before it finished.',
  fixFirst: 'Fix first',
  findingsByFile: 'Findings by file',
  noFileAnchor: 'Repository-wide',
  noFindings: 'No findings recorded in this run.',
  noFilteredFindings: 'No findings match the current filters.',
  filterSeverity: 'Severity',
  filterCategory: 'Category',
  filterFilePlaceholder: 'Filter by file path...',
  allSeverities: 'All severities',
  allCategories: 'All categories',
  clearFilters: 'Clear filters',
  copyPromptAll: 'Copy agent prompt',
  copyPromptOne: 'Copy agent prompt',
  copyPatchAll: 'Copy patch',
  copyPatchOne: 'Copy patch',
  copied: 'Copied',
  copyRefused: 'Could not copy; select the text below by hand',
  sourceUnavailable: 'Source unavailable',
  openOnGithub: 'View on GitHub',
  openOnGitlab: 'View on GitLab',
  changeDiff: 'Reviewed change',
  suggestedFix: 'Suggested fix',
  deleteLines: 'Delete these lines.',
  when: 'When',
  consequence: 'Consequence',
  alsoOnLines: 'Also on these lines',
  suggested: 'Suggested.',
  remediation: 'Rule guidance.',
  toCheck: 'To check.',
  limitation: 'Limitation.',
  noEofNewline: 'No newline at end of file',
  gapsHeading: 'Coverage gaps and unknowns',
  noGaps: 'No coverage gaps or unknown obligations.',
  unknownObligations: 'Unknown obligations',
  requiredTag: 'required',
  unreviewedPaths: 'Excluded or unreviewed paths',
  hiddenInChange: 'Marked generated in this change:',
  hiddenInChangeNote: 'its own content marks it generated; check it by hand',
  openQuestions: 'Open questions',
  unresolvedCandidates: 'Candidates left unresolved by the verifier',
  architectureHeading: 'Architecture',
  baseChecked: 'diff base checked',
  baseNotChecked: 'diff base not checked',
  fixedInHead: 'Fixed on head',
  policyHeading: 'Policy',
  policySource: 'Source',
  policyProposed: 'Proposed on head (not applied)',
  obligationsHeading: 'Obligations',
  evidenceHeading: 'Evidence',
  scopeHeading: 'Reviewed scope',
  advisoryNote: 'Advisory: Sych never approves, requests changes or gates merges.',
  sideOld: 'Base',
  sideNew: 'Head',
  expandLines: (count) => `Show all ${count} lines`,
  collapseLines: 'Collapse extra lines',
}

const RU: ReportStrings = {
  reportTitle: 'Ревью Sych',
  revision: 'ревизия',
  target: 'Цель',
  diffBase: 'База сравнения',
  head: 'Голова',
  branch: 'Ветка',
  stagedHead: 'индекс Git (не закоммичено)',
  worktreeHead: 'рабочее дерево (не закоммичено)',
  snapshotExpired: 'Снэпшот исходников удалён по сроку хранения; замечания показаны из результата.',
  costKnown: 'учтено',
  unknownCostCalls: 'с неизвестной стоимостью',
  modelCalls: 'Вызовы моделей',
  tokensInOut: 'Токены вход / выход',
  engine: 'Движок',
  limitReached: 'Лимит бюджета или размера остановил часть проверки до её завершения.',
  fixFirst: 'Исправить в первую очередь',
  findingsByFile: 'Замечания по файлам',
  noFileAnchor: 'Без привязки к файлу',
  noFindings: 'В этом прогоне замечаний не зафиксировано.',
  noFilteredFindings: 'По выбранным фильтрам замечаний нет.',
  filterSeverity: 'Критичность',
  filterCategory: 'Категория',
  filterFilePlaceholder: 'Фильтр по пути файла...',
  allSeverities: 'Все уровни',
  allCategories: 'Все категории',
  clearFilters: 'Сбросить фильтры',
  copyPromptAll: 'Скопировать промпт для агента',
  copyPromptOne: 'Скопировать промпт для агента',
  copyPatchAll: 'Скопировать патч',
  copyPatchOne: 'Скопировать патч',
  copied: 'Скопировано',
  copyRefused: 'Не удалось скопировать: выделите текст ниже вручную',
  sourceUnavailable: 'Исходный код недоступен',
  openOnGithub: 'Открыть на GitHub',
  openOnGitlab: 'Открыть в GitLab',
  changeDiff: 'Изменение в ревью',
  suggestedFix: 'Предлагаемое исправление',
  deleteLines: 'Удалить эти строки.',
  when: 'Когда',
  consequence: 'Последствие',
  alsoOnLines: 'Также на этих строках',
  suggested: 'Что сделать.',
  remediation: 'Рекомендация правила.',
  toCheck: 'Как проверить.',
  limitation: 'Ограничение.',
  noEofNewline: 'Нет перевода строки в конце файла',
  gapsHeading: 'Пробелы покрытия и неизвестные',
  noGaps: 'Пробелов покрытия и открытых обязательств нет.',
  unknownObligations: 'Неизвестные обязательства',
  requiredTag: 'обязательное',
  unreviewedPaths: 'Исключённые и непроверенные пути',
  hiddenInChange: 'Помечено сгенерированным в этом изменении:',
  hiddenInChangeNote: 'файл сам пометил себя сгенерированным; проверьте его вручную',
  openQuestions: 'Открытые вопросы',
  unresolvedCandidates: 'Кандидаты, оставшиеся неподтверждёнными',
  architectureHeading: 'Архитектура',
  baseChecked: 'база проверена',
  baseNotChecked: 'база не проверена',
  fixedInHead: 'Исправлено на head',
  policyHeading: 'Политика',
  policySource: 'Источник',
  policyProposed: 'Предложено в head (не применено)',
  obligationsHeading: 'Обязательства',
  evidenceHeading: 'Доказательства',
  scopeHeading: 'Область проверки',
  advisoryNote: 'Рекомендательный отчёт: Sych сам не одобряет PR и не блокирует слияние.',
  sideOld: 'Было',
  sideNew: 'Стало',
  expandLines: (count) => `Показать все строки (${count})`,
  collapseLines: 'Свернуть лишние строки',
}

export function strings(lang: Lang = 'en'): ReportStrings {
  return lang === 'ru' ? RU : EN
}
