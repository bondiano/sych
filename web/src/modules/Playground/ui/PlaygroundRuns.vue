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

defineProps<{ runs: Schemas['RunStatus'][] }>()
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Run</TableHead>
        <TableHead>Pull request</TableHead>
        <TableHead>State</TableHead>
        <TableHead>Coverage</TableHead>
        <TableHead class="text-right">Cost</TableHead>
        <TableHead>Started</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableRow v-for="r in runs" :key="r.run_id">
        <TableCell>
          <RouterLink :to="`/runs/${r.run_id}`" class="font-mono hover:underline">
            {{ r.run_id.slice(0, 8) }}
          </RouterLink>
        </TableCell>
        <TableCell>
          <RouterLink :to="`/changes/${r.change_id}`" class="hover:underline">Open</RouterLink>
        </TableCell>
        <TableCell><StatusBadge :value="r.state" /></TableCell>
        <TableCell><StatusBadge :value="r.coverage" /></TableCell>
        <TableCell class="text-right tabular-nums">
          {{ formatMicroUsd(r.usage.cost_known_micro_usd) }}
          <span v-if="r.usage.unknown_cost_attempts" class="text-muted-foreground">
            + {{ r.usage.unknown_cost_attempts }} unknown
          </span>
        </TableCell>
        <TableCell class="text-muted-foreground">{{ formatTime(r.created_at) }}</TableCell>
      </TableRow>
      <TableEmpty v-if="!runs.length" :colspan="6">No playground runs yet.</TableEmpty>
    </TableBody>
  </Table>
</template>
