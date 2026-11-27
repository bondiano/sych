<script setup lang="ts">
import { computed, ref } from 'vue'

import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import type { Schemas } from '@/modules/Api'
import {
  agentPrompt,
  categoryLabel,
  CopyPrompt,
  hex,
  place,
  ProseText,
  RunVerdict,
  safeText,
  strings,
} from '@/modules/Runs'
import { StatusBadge } from '@/modules/Ui'

import ReportArchitecture from './ReportArchitecture.vue'
import ReportCollapsedDetails from './ReportCollapsedDetails.vue'
import ReportFilters from './ReportFilters.vue'
import ReportFindingCard from './ReportFindingCard.vue'
import ReportGapsSection from './ReportGapsSection.vue'

const props = defineProps<{
  view: Schemas['ReportView'] | null
}>()

const severityFilter = ref<Schemas['Severity'] | ''>('')
const categoryFilter = ref<Schemas['FindingCategory'] | ''>('')
const fileQuery = ref('')

const lang = computed(() => props.view?.header.language ?? 'en')
const t = computed(() => strings(lang.value))
const snapshot = computed(() => props.view?.header.snapshot ?? null)
const links = computed(() => props.view?.header.code_links ?? undefined)

const headKindLabel = computed(() => {
  const kind = snapshot.value?.head_kind
  if (kind === 'STAGED') {
    return t.value.stagedHead
  }

  if (kind === 'WORKTREE') {
    return t.value.worktreeHead
  }

  return undefined
})

const openFindings = computed(() =>
  (props.view?.findings ?? []).filter((f) => f.lifecycle === 'OPEN'),
)

const filteredFindings = computed(() => {
  const q = fileQuery.value.trim().toLowerCase()
  return (props.view?.findings ?? []).filter((f) => {
    if (severityFilter.value && f.severity !== severityFilter.value) {
      return false
    }

    if (categoryFilter.value && f.category !== categoryFilter.value) {
      return false
    }

    if (q) {
      const matchesPath = f.anchors.some((a) => a.reference.path.toLowerCase().includes(q))
      if (!matchesPath) {
        return false
      }
    }

    return true
  })
})

interface FileGroup {
  key: string
  label: string
  findings: Schemas['ReportFinding'][]
}

const groupedByFile = computed<FileGroup[]>(() => {
  const map = new Map<string, Schemas['ReportFinding'][]>()
  for (const f of filteredFindings.value) {
    const file = f.anchors[0]?.reference.path ?? ''
    const list = map.get(file)
    if (list) {
      list.push(f)
    } else {
      map.set(file, [f])
    }
  }
  const groups: FileGroup[] = []
  for (const [key, findings] of map.entries()) {
    groups.push({
      key: key || '__repo__',
      label: key ? safeText(key) : t.value.noFileAnchor,
      findings,
    })
  }
  return groups
})

const numberFmt = new Intl.NumberFormat('en-US')

function resetFilters() {
  severityFilter.value = ''
  categoryFilter.value = ''
  fileQuery.value = ''
}
</script>

<template>
  <main class="mx-auto max-w-6xl space-y-8 px-4 py-8 sm:px-6 lg:px-8">
    <p v-if="!view" class="rounded-lg border bg-card p-6 text-sm text-muted-foreground">
      Report payload is missing or invalid.
    </p>

    <template v-else>
      <header class="space-y-4">
        <div class="flex flex-wrap items-baseline justify-between gap-3">
          <h1 class="cn-font-heading text-2xl font-semibold tracking-tight">
            {{ t.reportTitle }}
            <span
              v-if="snapshot?.head_ref"
              class="font-mono text-lg font-normal text-muted-foreground"
            >
              · {{ safeText(snapshot.head_ref) }}
            </span>
          </h1>
          <span class="font-mono text-xs text-muted-foreground tabular-nums">
            {{ view.header.run_id.slice(0, 8) }} · {{ t.revision }}
            {{ view.header.review_revision }}
          </span>
        </div>

        <div class="flex flex-wrap items-center gap-2 text-xs">
          <StatusBadge :value="view.header.recommendation" />
          <StatusBadge :value="view.header.coverage" />
          <StatusBadge :value="view.header.execution_outcome" />
          <span
            v-if="headKindLabel"
            class="rounded-md border border-amber-300/80 bg-amber-50 px-2 py-0.5 font-medium text-amber-950 dark:border-amber-800/70 dark:bg-amber-950/70 dark:text-amber-100"
          >
            {{ headKindLabel }}
          </span>
        </div>

        <dl
          class="flex flex-wrap gap-x-6 gap-y-2 rounded-lg border bg-card px-4 py-3 text-xs text-muted-foreground"
        >
          <template v-if="snapshot">
            <div v-if="snapshot.target_ref" class="flex items-center gap-1.5">
              <dt>{{ t.target }}:</dt>
              <dd class="font-mono text-foreground">
                {{ safeText(snapshot.target_ref) }} ({{ hex(snapshot.target_tip).slice(0, 12) }})
              </dd>
            </div>
            <div class="flex items-center gap-1.5">
              <dt>{{ t.diffBase }}:</dt>
              <dd class="font-mono text-foreground">{{ hex(snapshot.diff_base).slice(0, 12) }}</dd>
            </div>
            <div class="flex items-center gap-1.5">
              <dt>{{ t.head }}:</dt>
              <dd class="font-mono text-foreground">{{ hex(snapshot.head).slice(0, 12) }}</dd>
            </div>
          </template>
          <p v-else>{{ t.snapshotExpired }}</p>

          <div class="flex items-center gap-1.5 tabular-nums">
            <dt>{{ t.modelCalls }}:</dt>
            <dd class="text-foreground">{{ view.header.usage.model_attempts }}</dd>
          </div>
          <div class="flex items-center gap-1.5 tabular-nums">
            <dt>{{ t.tokensInOut }}:</dt>
            <dd class="text-foreground">
              {{ numberFmt.format(view.header.usage.input_tokens) }} /
              {{ numberFmt.format(view.header.usage.output_tokens) }}
            </dd>
          </div>
          <div class="flex items-center gap-1.5 tabular-nums">
            <dt>Cost:</dt>
            <dd class="text-foreground">
              {{ formatMicroUsd(view.header.usage.cost.known) }} {{ t.costKnown }}
              <span
                v-if="view.header.usage.cost.unknown_cost_attempts"
                class="text-muted-foreground"
              >
                (+{{ view.header.usage.cost.unknown_cost_attempts }} {{ t.unknownCostCalls }})
              </span>
            </dd>
          </div>
          <div class="flex items-center gap-1.5">
            <dt>{{ t.engine }}:</dt>
            <dd class="font-mono text-foreground">
              {{ safeText(view.header.provenance.engine_version) }}
            </dd>
          </div>
        </dl>
      </header>

      <!-- 1. Verdict and coverage -->
      <section class="space-y-3">
        <RunVerdict :view="view" :language="lang" />
        <div
          v-if="view.header.counts.length || openFindings.length || view.fixes_patch"
          class="flex flex-wrap items-center justify-between gap-3"
        >
          <ul class="flex flex-wrap items-center gap-2 text-xs">
            <li
              v-for="c in view.header.counts"
              :key="`${c.severity}:${c.category}`"
              class="inline-flex items-center gap-1.5 rounded-md border bg-card px-2.5 py-1 tabular-nums"
            >
              <StatusBadge :value="c.severity" />
              <span>{{ categoryLabel(c.category, lang) }}</span>
              <span class="font-semibold">{{ c.count }}</span>
            </li>
          </ul>
          <div class="flex flex-wrap items-start gap-2 print:hidden">
            <CopyPrompt
              v-if="openFindings.length"
              :text="agentPrompt(openFindings, lang)"
              :label="t.copyPromptAll"
              :language="lang"
            />
            <CopyPrompt
              v-if="view.fixes_patch"
              :text="view.fixes_patch"
              :label="t.copyPatchAll"
              :language="lang"
            />
          </div>
        </div>
      </section>

      <!-- 2. Fix first priority list -->
      <section v-if="openFindings.length" class="space-y-3">
        <h2 class="text-lg font-semibold">{{ t.fixFirst }}</h2>
        <ol class="divide-y rounded-lg border bg-card text-sm">
          <li
            v-for="(f, idx) in openFindings"
            :key="f.id"
            class="flex flex-wrap items-baseline justify-between gap-2 px-4 py-2.5"
          >
            <div class="flex min-w-0 flex-wrap items-baseline gap-2">
              <span class="font-mono text-xs text-muted-foreground tabular-nums"
                >{{ idx + 1 }}.</span
              >
              <StatusBadge :value="f.severity" />
              <span class="text-xs text-muted-foreground">
                {{ categoryLabel(f.category, lang) }}
              </span>
              <a
                :href="`#finding-${f.id}`"
                class="font-medium underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
              >
                <ProseText :text="f.title" />
              </a>
            </div>
            <span v-if="f.anchors[0]" class="font-mono text-xs break-all text-muted-foreground">
              {{ place(f.anchors[0].reference) }}
            </span>
          </li>
        </ol>
      </section>

      <!-- 3. Findings grouped by file, with filters -->
      <section data-section="findings" class="space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <h2 class="text-lg font-semibold">{{ t.findingsByFile }}</h2>
          <ReportFilters
            v-if="view.findings.length"
            v-model:severity="severityFilter"
            v-model:category="categoryFilter"
            v-model:file-query="fileQuery"
            :language="lang"
            @reset="resetFilters"
          />
        </div>

        <p
          v-if="!view.findings.length"
          class="rounded-lg border bg-card p-4 text-sm text-muted-foreground"
        >
          {{ t.noFindings }}
        </p>
        <p
          v-else-if="!groupedByFile.length"
          class="rounded-lg border bg-card p-4 text-sm text-muted-foreground"
        >
          {{ t.noFilteredFindings }}
        </p>

        <div v-else class="space-y-6">
          <div v-for="group in groupedByFile" :key="group.key" class="space-y-2">
            <h3 class="flex items-center gap-2 font-mono text-sm font-medium break-all">
              <span>{{ group.label }}</span>
              <span class="font-sans text-xs font-normal text-muted-foreground tabular-nums">
                ({{ group.findings.length }})
              </span>
            </h3>
            <div class="divide-y rounded-lg border bg-card">
              <ReportFindingCard
                v-for="f in group.findings"
                :key="f.id"
                :finding="f"
                :links="links"
                :language="lang"
              />
            </div>
          </div>
        </div>
      </section>

      <!-- 4. Gaps (as visible as findings) -->
      <ReportGapsSection :gaps="view.gaps" :language="lang" />

      <!-- 5. Architecture pictures -->
      <ReportArchitecture
        :projects="view.architecture ?? []"
        :findings="view.findings"
        :links="links"
        :language="lang"
      />

      <!-- 6. Collapsed Policy, Obligations, Evidence and Scope -->
      <ReportCollapsedDetails :view="view" :links="links" :language="lang" />

      <footer class="border-t pt-4 text-xs text-muted-foreground">
        {{ t.advisoryNote }}
      </footer>
    </template>
  </main>
</template>
