<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import { useChange } from '@/modules/Changes'
import { useFindings } from '@/modules/Findings'
import {
  agentPrompt,
  AttemptsTable,
  CandidateList,
  CopyPrompt,
  RunDisclosure,
  FindingArticle,
  hex,
  optional,
  problems,
  RunChecks,
  RunFacts,
  RunScope,
  RunVerdict,
  useAttempts,
  useResult,
  useRun,
  useSnapshots,
} from '@/modules/Runs'
import { ErrorNotice, Skeleton, statusLabel, StatusBadge } from '@/modules/Ui'

const route = useRoute()
const id = computed(() => String(route.params.id))
const run = useRun(id)
const result = useResult(id)
const attempts = useAttempts(id)
const snapshots = useSnapshots(id)
const changeId = computed(() => run.data.value?.change_id ?? 0)
const change = useChange(changeId)
const findings = useFindings(computed(() => ({ change_id: changeId.value })))

const pull = computed(() => change.data.value?.change)
const repo = computed(() => pull.value?.repository.full_name)
const links = computed(() => {
  const source = snapshots.data.value?.source
  return source && repo.value
    ? { repo: repo.value, head: hex(source.head), base: hex(source.diff_base) }
    : undefined
})
const found = computed(() => (result.data.value ? problems(result.data.value) : []))
const minor = computed(() => (result.data.value ? optional(result.data.value) : []))
const checked = computed(() => {
  const r = result.data.value
  return (
    !!r &&
    r.obligations.length + r.excluded_or_unreviewed.length + r.unresolved_questions.length > 0
  )
})
// shortcut: only the change's newest page of findings gets a link, add a run filter if one outgrows it.
const tracked = computed(
  () =>
    new Map(
      findings.data.value?.pages.flatMap((p) => p.items.map((f) => [f.finding_id, f] as const)),
    ),
)
</script>

<template>
  <ErrorNotice v-if="run.error.value" :error="run.error.value" />
  <Skeleton v-else-if="!run.data.value" class="h-40 w-full" />
  <div v-else class="space-y-6">
    <header class="space-y-2">
      <h1 class="text-xl font-semibold">
        <RouterLink v-if="pull" :to="`/changes/${pull.id}`" class="hover:underline">
          {{ repo }}<template v-if="pull.number">#{{ pull.number }}</template>
        </RouterLink>
        <template v-else>Run</template>
        <span class="font-normal text-muted-foreground">
          · revision {{ run.data.value.generation }}
        </span>
      </h1>
      <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <StatusBadge :value="run.data.value.state" />
        <span v-if="!run.data.value.finished_at" class="text-muted-foreground">
          {{ statusLabel(run.data.value.stage) }}
        </span>
        <a
          v-if="repo && pull?.number"
          :href="`https://github.com/${repo}/pull/${pull.number}`"
          target="_blank"
          rel="noopener noreferrer"
          class="underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
          >Pull request on GitHub</a
        >
        <RouterLink
          :to="`/runs/${id}/tour`"
          class="underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
        >
          Tour
        </RouterLink>
      </div>
      <p v-if="run.data.value.failure" class="text-sm text-destructive">
        {{ run.data.value.failure }}
      </p>
    </header>

    <ErrorNotice v-if="result.error.value" :error="result.error.value" />
    <RunVerdict v-else-if="result.data.value" :result="result.data.value" />
    <p v-else-if="result.data.value === null" class="text-sm text-muted-foreground">
      <template v-if="run.data.value.finished_at">
        No result to show: the run did not commit one, or retention deleted it.
      </template>
      <template v-else>The review is still running; its result appears here when it ends.</template>
    </p>

    <div class="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_19rem]">
      <div class="min-w-0 space-y-8">
        <template v-if="result.data.value">
          <section v-if="found.length" class="space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <h2 class="text-lg font-medium">Problems</h2>
              <CopyPrompt :text="agentPrompt(found)" label="Copy prompt for all" />
            </div>
            <div class="divide-y rounded-lg border bg-card">
              <FindingArticle
                v-for="f in found"
                :key="f.id"
                :finding="f"
                :links="links"
                :tracked="tracked.get(f.id)"
              />
            </div>
          </section>

          <section v-if="checked" class="space-y-3">
            <h2 class="text-lg font-medium">Checks</h2>
            <RunChecks :result="result.data.value" />
          </section>

          <div class="space-y-2">
            <RunDisclosure
              v-if="minor.length"
              title="Improvements and nitpicks"
              :count="minor.length"
            >
              <div class="divide-y">
                <FindingArticle
                  v-for="f in minor"
                  :key="f.id"
                  :finding="f"
                  :links="links"
                  :tracked="tracked.get(f.id)"
                />
              </div>
            </RunDisclosure>
            <RunDisclosure
              v-if="result.data.value.unresolved_candidates?.length"
              title="Candidates the verifier left open"
              :count="result.data.value.unresolved_candidates.length"
            >
              <CandidateList :candidates="result.data.value.unresolved_candidates" :links="links" />
            </RunDisclosure>
            <RunDisclosure
              v-if="result.data.value.rejected_candidates?.length"
              title="Candidates the verifier rejected"
              :count="result.data.value.rejected_candidates.length"
            >
              <CandidateList :candidates="result.data.value.rejected_candidates" :links="links" />
            </RunDisclosure>
            <RunDisclosure
              title="Reviewed scope"
              :count="result.data.value.scope_manifest.units.length"
            >
              <RunScope :scope="result.data.value.scope_manifest" />
            </RunDisclosure>
          </div>
        </template>
      </div>

      <aside class="space-y-3 lg:sticky lg:top-6">
        <RunFacts :run="run.data.value" :result="result.data.value" :links="links" />
        <p class="px-1 text-xs text-muted-foreground">
          Advisory: Sych never approves, requests changes or gates merges.
        </p>
      </aside>
    </div>

    <section class="space-y-3">
      <h2 class="text-lg font-medium">Model calls</h2>
      <ErrorNotice v-if="attempts.error.value" :error="attempts.error.value" />
      <AttemptsTable v-else-if="attempts.data.value" :attempts="attempts.data.value" />
    </section>
  </div>
</template>
