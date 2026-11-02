<script setup lang="ts">
import { computed } from 'vue'

import { formatAge } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/modules/Ui'

const props = defineProps<{ latency: Schemas['Latency'] }>()

const tiles = computed(() => [
  { title: 'To delivery', spread: props.latency.to_delivery, note: 'run created to first comment' },
  { title: 'Run duration', spread: props.latency.run_duration, note: 'created to completed' },
  {
    title: 'Queue wait',
    spread: props.latency.queue_wait,
    note: 'job created to first claim; jobs are kept 30 days',
  },
])
const show = (s: number | null | undefined) => (s == null ? '—' : formatAge(s))
</script>

<template>
  <div class="grid gap-4 sm:grid-cols-3">
    <Card v-for="t in tiles" :key="t.title">
      <CardHeader>
        <CardDescription>{{ t.title }}</CardDescription>
        <CardTitle class="text-2xl tabular-nums"
          >{{ show(t.spread.p50_seconds) }}
          <span class="text-sm font-normal text-muted-foreground"
            >median · p90 {{ show(t.spread.p90_seconds) }}</span
          ></CardTitle
        >
      </CardHeader>
      <CardContent class="text-sm text-muted-foreground"
        >{{ t.note }}; n = {{ t.spread.n }}</CardContent
      >
    </Card>
  </div>
</template>
