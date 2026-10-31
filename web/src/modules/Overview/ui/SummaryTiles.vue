<script setup lang="ts">
import { computed } from 'vue'

import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import { formatAge } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/modules/Ui'

const props = defineProps<{ summary: Schemas['Summary'] }>()

const count = (rows: Schemas['Count'][], key: string) => rows.find((r) => r.key === key)?.count ?? 0
const total = (rows: Schemas['Count'][]) => rows.reduce((sum, r) => sum + r.count, 0)

const tiles = computed(() => {
  const s = props.summary
  const runs = total(s.runs_by_state)
  const partial = count(s.runs_by_coverage, 'PARTIAL') + count(s.runs_by_coverage, 'UNAVAILABLE')
  return [
    {
      title: 'Runs',
      value: String(runs),
      note: `${count(s.runs_by_state, 'COMPLETED')} completed · ${count(s.runs_by_state, 'FAILED')} failed`,
    },
    {
      title: 'Incomplete coverage',
      value: String(partial),
      note: `of ${total(s.runs_by_coverage)} finished runs: gaps were left unchecked`,
    },
    {
      title: 'Unknown obligations',
      value: String(s.unknown_obligations),
      note: 'required checks the reviewer could not settle',
    },
    {
      title: 'Findings',
      value: String(count(s.finding_events, 'OPEN')),
      note: `found · ${count(s.finding_events, 'RESOLVED')} resolved · ${count(s.finding_events, 'DISMISSED')} dismissed`,
    },
    {
      title: 'Model cost',
      value: formatMicroUsd(s.cost_known_micro_usd),
      note: s.unknown_cost_attempts
        ? `known; plus ${s.unknown_cost_attempts} calls of unknown cost`
        : 'every call priced',
    },
    {
      title: 'Queue',
      value: String(s.queued_jobs),
      note: s.queued_jobs
        ? `oldest waits ${formatAge(s.oldest_queued_age_seconds)}`
        : `${s.uncertain_publications} publications of unknown outcome`,
    },
  ]
})
</script>

<template>
  <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
    <Card v-for="t in tiles" :key="t.title">
      <CardHeader>
        <CardDescription>{{ t.title }}</CardDescription>
        <CardTitle class="text-2xl tabular-nums">{{ t.value }}</CardTitle>
      </CardHeader>
      <CardContent class="text-sm text-muted-foreground">{{ t.note }}</CardContent>
    </Card>
  </div>
</template>
