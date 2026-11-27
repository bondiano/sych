<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import { ProseText, safeText, strings, type Lang } from '@/modules/Runs'
import {
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

const props = defineProps<{
  gaps: Schemas['ReportGaps']
  language?: Lang
}>()

const t = computed(() => strings(props.language))

const sortedUnknowns = computed(() =>
  [...props.gaps.unknown_obligations].sort((a, b) => Number(b.required) - Number(a.required)),
)
const hiddenPaths = computed(() =>
  props.gaps.paths.filter((p) => p.reason === 'GENERATED_IN_CHANGE'),
)
const otherPaths = computed(() =>
  props.gaps.paths.filter((p) => p.reason !== 'GENERATED_IN_CHANGE'),
)
const hasAnyGap = computed(
  () =>
    props.gaps.unknown_obligations.length +
      props.gaps.paths.length +
      props.gaps.open_questions.length +
      props.gaps.unresolved_candidates.length >
    0,
)
</script>

<template>
  <section class="space-y-4">
    <h2 class="text-lg font-semibold">{{ t.gapsHeading }}</h2>

    <p v-if="!hasAnyGap" class="rounded-lg border bg-card p-4 text-sm text-muted-foreground">
      {{ t.noGaps }}
    </p>

    <div v-else class="space-y-4 rounded-lg border bg-card p-5">
      <div v-if="sortedUnknowns.length" class="space-y-2">
        <h3 class="text-sm font-medium">{{ t.unknownObligations }}</h3>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Rule</TableHead>
              <TableHead>Question</TableHead>
              <TableHead>Reason</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="u in sortedUnknowns" :key="u.id">
              <TableCell class="font-mono text-xs whitespace-nowrap">
                {{ safeText(u.rule_id) }}
                <span v-if="u.required" class="ml-1 text-muted-foreground">
                  ({{ t.requiredTag }})
                </span>
              </TableCell>
              <TableCell class="max-w-md whitespace-normal">
                <ProseText :text="u.question" />
              </TableCell>
              <TableCell class="max-w-sm text-sm whitespace-normal text-muted-foreground">
                <span class="font-mono text-xs text-foreground">{{ safeText(u.reason_code) }}</span
                >: <ProseText :text="u.next_evidence_needed" />
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <div v-if="hiddenPaths.length || otherPaths.length" class="space-y-2">
        <h3 class="text-sm font-medium">{{ t.unreviewedPaths }}</h3>
        <ul class="space-y-1.5 text-sm">
          <li
            v-for="g in hiddenPaths"
            :key="`hidden-${g.path}`"
            class="rounded-md border border-amber-300/80 bg-amber-50/70 px-3 py-2 dark:border-amber-800/70 dark:bg-amber-950/40"
          >
            <span class="font-medium">{{ t.hiddenInChange }}</span>
            <span class="ml-1.5 font-mono text-xs break-all">{{ safeText(g.path) }}</span>
            <span class="ml-1 text-muted-foreground">({{ t.hiddenInChangeNote }})</span>
          </li>
          <li v-for="g in otherPaths" :key="`${g.path}-${g.reason}`">
            <span class="font-mono text-xs break-all">{{ safeText(g.path) }}</span>
            <span class="ml-1.5 text-muted-foreground">{{ safeText(g.reason) }}</span>
          </li>
        </ul>
      </div>

      <div v-if="gaps.open_questions.length" class="space-y-2">
        <h3 class="text-sm font-medium">{{ t.openQuestions }}</h3>
        <ul class="space-y-1 text-sm text-muted-foreground">
          <li v-for="q in gaps.open_questions" :key="q">
            <ProseText :text="q" />
          </li>
        </ul>
      </div>

      <div v-if="gaps.unresolved_candidates.length" class="space-y-2">
        <h3 class="text-sm font-medium">{{ t.unresolvedCandidates }}</h3>
        <ul class="space-y-1.5 text-sm">
          <li v-for="c in gaps.unresolved_candidates" :key="c.id" class="space-y-0.5">
            <div class="flex items-center gap-2">
              <StatusBadge :value="c.verification_status" />
              <span class="font-mono text-xs text-muted-foreground">{{ c.id.slice(0, 8) }}</span>
            </div>
            <p v-for="lim in c.limitations" :key="lim" class="text-xs text-muted-foreground">
              <ProseText :text="lim" />
            </p>
          </li>
        </ul>
      </div>
    </div>
  </section>
</template>
