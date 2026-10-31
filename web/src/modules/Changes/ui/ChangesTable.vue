<script setup lang="ts">
import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import {
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

defineProps<{ changes: Schemas['ChangeSummary'][] }>()
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Pull request</TableHead>
        <TableHead>State</TableHead>
        <TableHead>Latest run</TableHead>
        <TableHead>Coverage</TableHead>
        <TableHead class="text-right">Open findings</TableHead>
        <TableHead class="text-right">Cost</TableHead>
        <TableHead>Reviewed</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!changes.length" :colspan="7">No pull requests.</TableEmpty>
      <TableRow v-for="c in changes" :key="c.id">
        <TableCell>
          <RouterLink :to="`/changes/${c.id}`" class="font-medium hover:underline">
            {{ c.repository.full_name }}#{{ c.number ?? '?' }}
          </RouterLink>
        </TableCell>
        <TableCell class="text-muted-foreground lowercase">{{ c.scm_state ?? '—' }}</TableCell>
        <TableCell><StatusBadge :value="c.latest_run?.state" /></TableCell>
        <TableCell><StatusBadge :value="c.latest_run?.coverage" /></TableCell>
        <TableCell class="text-right tabular-nums">{{ c.open_findings }}</TableCell>
        <TableCell class="text-right tabular-nums">
          <template v-if="c.latest_run">
            {{ formatMicroUsd(c.latest_run.cost_known_micro_usd) }}
            <span v-if="c.latest_run.unknown_cost_attempts" class="text-muted-foreground">
              + {{ c.latest_run.unknown_cost_attempts }} unknown
            </span>
          </template>
          <template v-else>—</template>
        </TableCell>
        <TableCell class="text-muted-foreground">
          {{ formatTime(c.latest_run?.finished_at ?? c.latest_run?.created_at) }}
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
