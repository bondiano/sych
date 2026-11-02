<script setup lang="ts">
import { computed, ref } from 'vue'

import { formatShare } from '@/common/utilities/formatShare'
import type { Schemas } from '@/modules/Api'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

import { breakdown, DIMENSIONS, type Dimension, noisyRules } from '../judgements'

const props = defineProps<{ quality: Schemas['Quality'] }>()

const LABELS: Record<Dimension, string> = {
  rule_id: 'Rule',
  category: 'Category',
  severity: 'Severity',
  repository: 'Repository',
}
const by = ref<Dimension>('rule_id')

const settled = computed(() => {
  const count = (key: string) => props.quality.settled.find((c) => c.key === key)?.count ?? 0
  return {
    resolved: count('RESOLVED'),
    total: props.quality.settled.reduce((s, c) => s + c.count, 0),
  }
})
const all = computed(() => breakdown(props.quality.judgements, 'repository'))
const judged = computed(() => all.value.reduce((s, r) => s + r.total, 0))
const falsePositive = computed(() => all.value.reduce((s, r) => s + r.falsePositive, 0))
const rows = computed(() => breakdown(props.quality.judgements, by.value))
const noisy = computed(() => noisyRules(props.quality.judgements))
const merged = computed(() =>
  props.quality.merged_with_open.map((m) => `${m.changes} with ${m.severity ?? 'unknown'}`),
)
</script>

<template>
  <div class="space-y-6">
    <div class="grid gap-4 sm:grid-cols-3">
      <Card>
        <CardHeader>
          <CardDescription>Addressed</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{
            formatShare(settled.resolved, settled.total)
          }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          {{ settled.resolved }} resolved of {{ settled.total }} findings of merged or closed pull
          requests; resolved means a later run checked the fix
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardDescription>False positives</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{
            formatShare(falsePositive, judged)
          }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          {{ falsePositive }} of {{ judged }} judgements, each person's latest per finding
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardDescription>Merged with open findings</CardDescription>
          <CardTitle class="text-2xl tabular-nums">{{
            quality.merged_with_open.length ? merged.join(' · ') : 'none'
          }}</CardTitle>
        </CardHeader>
        <CardContent class="text-sm text-muted-foreground">
          pull requests by the severity they left open
        </CardContent>
      </Card>
    </div>

    <section>
      <div class="mb-3 flex items-center gap-3">
        <h2 class="mr-auto text-lg font-medium">Judgements</h2>
        <Select v-model="by">
          <SelectTrigger class="w-40" aria-label="Group by"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem v-for="d in DIMENSIONS" :key="d" :value="d">By {{ LABELS[d] }}</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ LABELS[by] }}</TableHead>
            <TableHead class="text-right">Judged</TableHead>
            <TableHead class="text-right">Useful</TableHead>
            <TableHead class="text-right">Not useful</TableHead>
            <TableHead class="text-right">False positive</TableHead>
            <TableHead class="text-right">Won't fix</TableHead>
            <TableHead class="text-right">False positive share</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableEmpty v-if="!rows.length" :colspan="7">No feedback in this period.</TableEmpty>
          <TableRow v-for="r in rows" :key="r.key">
            <TableCell class="font-mono text-xs">{{ r.key }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.total }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.useful }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.notUseful }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.falsePositive }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.wontFix }}</TableCell>
            <TableCell class="text-right tabular-nums">{{
              formatShare(r.falsePositive, r.total)
            }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>

    <section>
      <h2 class="mb-1 text-lg font-medium">Noisy rules</h2>
      <p class="mb-3 text-sm text-muted-foreground">
        Rules judged at least five times, by false positive share. Sych never turns a rule off: fix
        or drop it by a pull request to <span class="font-mono">.sych/rules.yaml</span>.
      </p>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Rule</TableHead>
            <TableHead class="text-right">False positive</TableHead>
            <TableHead class="text-right">Judged</TableHead>
            <TableHead class="text-right">Share</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableEmpty v-if="!noisy.length" :colspan="4">No rule stands out yet.</TableEmpty>
          <TableRow v-for="r in noisy" :key="r.key">
            <TableCell class="font-mono text-xs">{{ r.key }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.falsePositive }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ r.total }}</TableCell>
            <TableCell class="text-right tabular-nums">{{
              formatShare(r.falsePositive, r.total)
            }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>
  </div>
</template>
