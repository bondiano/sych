<script setup lang="ts">
import { computed } from 'vue'

import { formatShare } from '@/common/utilities/formatShare'
import type { Schemas } from '@/modules/Api'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

const props = defineProps<{ honesty: Schemas['Honesty'] }>()

const day = (seconds: number) => new Date(seconds * 1000).toISOString().slice(0, 10)
const peak = computed(() => Math.max(1, ...props.honesty.coverage_by_day.map((d) => d.runs)))
const total = computed(() =>
  props.honesty.coverage_by_day.reduce(
    (t, d) => ({ runs: t.runs + d.runs, partial: t.partial + d.partial }),
    { runs: 0, partial: 0 },
  ),
)
const lists = computed(() => [
  { title: 'Runs by outcome', unit: 'runs', rows: props.honesty.outcomes },
  { title: 'Gaps by reason', unit: 'paths', rows: props.honesty.gaps },
  { title: 'Unknown obligations by reason', unit: 'obligations', rows: props.honesty.unknown },
])
</script>

<template>
  <div class="space-y-6">
    <Card>
      <CardHeader>
        <CardTitle>Incomplete coverage by day</CardTitle>
        <CardDescription>
          {{ total.partial }} of {{ total.runs }} finished runs ({{
            formatShare(total.partial, total.runs)
          }}) did not cover their whole scope; the darker part of each bar.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p v-if="!honesty.coverage_by_day.length" class="text-sm text-muted-foreground">
          No finished runs.
        </p>
        <ul v-else class="flex h-40 items-end gap-1" aria-label="Runs per day, incomplete part">
          <li
            v-for="d in honesty.coverage_by_day"
            :key="d.day"
            class="flex min-h-px flex-1 flex-col justify-end overflow-hidden rounded-t bg-primary/30"
            :style="{ height: `${(d.runs / peak) * 100}%` }"
            :title="`${day(d.day)}: ${d.partial} of ${d.runs} runs incomplete`"
          >
            <div class="bg-primary" :style="{ height: `${(d.partial / d.runs) * 100}%` }" />
          </li>
        </ul>
      </CardContent>
    </Card>
    <p v-if="honesty.unrecorded_runs" class="text-sm text-muted-foreground">
      {{ honesty.unrecorded_runs }} runs completed before reasons were recorded are in none of the
      tables below.
    </p>
    <div class="grid gap-6 lg:grid-cols-3">
      <section v-for="l in lists" :key="l.title">
        <h3 class="mb-2 font-medium">{{ l.title }}</h3>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Reason</TableHead>
              <TableHead class="text-right">{{ l.unit }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableEmpty v-if="!l.rows.length" :colspan="2">None.</TableEmpty>
            <TableRow v-for="r in l.rows" :key="r.key">
              <TableCell class="font-mono text-xs">{{ r.key }}</TableCell>
              <TableCell class="text-right tabular-nums">{{ r.count }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </section>
    </div>
  </div>
</template>
