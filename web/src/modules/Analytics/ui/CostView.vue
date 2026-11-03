<script setup lang="ts">
import { computed } from 'vue'

import { counted } from '@/common/utilities/counted'
import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
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

const props = defineProps<{ cost: Schemas['CostBreakdown'] }>()

const known = computed(() => props.cost.by_purpose.reduce((s, r) => s + r.cost_known_micro_usd, 0))
const unknown = computed(() =>
  props.cost.by_purpose.reduce((s, r) => s + r.unknown_cost_attempts, 0),
)
const perUseful = computed(() =>
  props.cost.useful_findings ? formatMicroUsd(known.value / props.cost.useful_findings) : '—',
)
const tables = computed(() => [
  { title: 'By repository', rows: props.cost.by_repository },
  { title: 'By model', rows: props.cost.by_model },
  { title: 'By purpose', rows: props.cost.by_purpose },
])
const used = (spent: number, limit: number | null | undefined) =>
  limit
    ? `${formatMicroUsd(spent)} of ${formatMicroUsd(limit)}`
    : `${formatMicroUsd(spent)}, no cap`
</script>

<template>
  <div class="space-y-6">
    <div class="grid gap-4 sm:grid-cols-3">
      <Card>
        <CardHeader>
          <CardDescription>Known cost</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{ formatMicroUsd(known) }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          {{ unknown ? `plus ${counted(unknown, 'call')} of unknown cost` : 'every call priced' }};
          {{ formatMicroUsd(cost.superseded_micro_usd) }} on revisions a newer push superseded
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardDescription>Per useful finding</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{ perUseful }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          known cost over {{ counted(cost.useful_findings, 'finding') }} someone called useful
        </CardContent>
      </Card>
      <Card v-if="cost.budget">
        <CardHeader>
          <CardDescription>Tenant budget</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{
            formatMicroUsd(cost.budget.month_forecast_micro_usd)
          }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          forecast for this month; spent
          {{ used(cost.budget.month_spent_micro_usd, cost.budget.month_limit_micro_usd) }} this
          month, {{ used(cost.budget.day_spent_micro_usd, cost.budget.day_limit_micro_usd) }} today,
          held reservations included
        </CardContent>
      </Card>
    </div>
    <p class="text-sm text-muted-foreground">
      Settled model calls are kept 90 days by default (<span class="font-mono"
        >storage.retention</span
      >); a longer period shows less than was spent.
    </p>
    <div class="grid gap-6 lg:grid-cols-3">
      <section v-for="t in tables" :key="t.title">
        <h3 class="mb-2 font-medium">{{ t.title }}</h3>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead />
              <TableHead class="text-right">Calls</TableHead>
              <TableHead class="text-right">Known</TableHead>
              <TableHead class="text-right">Unknown</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableEmpty v-if="!t.rows.length" :colspan="4">No calls.</TableEmpty>
            <TableRow v-for="r in t.rows" :key="r.key">
              <TableCell class="font-mono text-xs break-all">{{ r.key }}</TableCell>
              <TableCell class="text-right tabular-nums">{{ r.attempts }}</TableCell>
              <TableCell class="text-right tabular-nums">{{
                formatMicroUsd(r.cost_known_micro_usd)
              }}</TableCell>
              <TableCell class="text-right tabular-nums">{{ r.unknown_cost_attempts }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </section>
    </div>
    <section>
      <h3 class="mb-2 font-medium">Most expensive pull requests</h3>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Pull request</TableHead>
            <TableHead class="text-right">Calls</TableHead>
            <TableHead class="text-right">Known</TableHead>
            <TableHead class="text-right">Unknown</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableEmpty v-if="!cost.top_changes.length" :colspan="4">No calls.</TableEmpty>
          <TableRow v-for="c in cost.top_changes" :key="c.change_id">
            <TableCell>
              <RouterLink :to="`/changes/${c.change_id}`" class="hover:underline">
                {{ c.repository }}#{{ c.number ?? '?' }}
              </RouterLink>
            </TableCell>
            <TableCell class="text-right tabular-nums">{{ c.attempts }}</TableCell>
            <TableCell class="text-right tabular-nums">{{
              formatMicroUsd(c.cost_known_micro_usd)
            }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ c.unknown_cost_attempts }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>
  </div>
</template>
