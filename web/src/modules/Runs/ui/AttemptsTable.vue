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

defineProps<{ attempts: Schemas['Attempt'][] }>()

const tokens = (n: number | null | undefined) => (n === null || n === undefined ? '?' : String(n))
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Purpose</TableHead>
        <TableHead>Model</TableHead>
        <TableHead>Ledger</TableHead>
        <TableHead class="text-right">Tokens in / out</TableHead>
        <TableHead class="text-right">Charge</TableHead>
        <TableHead>At</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!attempts.length" :colspan="6">No model calls.</TableEmpty>
      <TableRow v-for="a in attempts" :key="a.id">
        <TableCell>
          {{ a.purpose }} <span class="text-muted-foreground">· {{ a.kind }}</span>
        </TableCell>
        <TableCell class="font-mono text-xs">
          {{ a.resolved_model ?? a.requested_model }}
          <span class="text-muted-foreground">({{ a.profile }})</span>
        </TableCell>
        <TableCell><StatusBadge :value="a.state" /></TableCell>
        <TableCell class="text-right tabular-nums">
          {{ tokens(a.input_tokens) }} / {{ tokens(a.output_tokens) }}
        </TableCell>
        <TableCell class="text-right tabular-nums">
          {{ formatMicroUsd(a.charge_micro_usd) }}
        </TableCell>
        <TableCell class="text-muted-foreground">{{ formatTime(a.created_at) }}</TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
