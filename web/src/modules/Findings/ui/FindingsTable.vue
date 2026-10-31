<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import {
  statusLabel,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

defineProps<{ findings: Schemas['FindingSummary'][]; showChange?: boolean }>()
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Severity</TableHead>
        <TableHead>Finding</TableHead>
        <TableHead v-if="showChange">Pull request</TableHead>
        <TableHead>Where</TableHead>
        <TableHead>State</TableHead>
        <TableHead>Updated</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!findings.length" :colspan="showChange ? 6 : 5">No findings.</TableEmpty>
      <TableRow v-for="f in findings" :key="f.id">
        <TableCell><StatusBadge :value="f.severity" /></TableCell>
        <TableCell class="max-w-md whitespace-normal">
          <RouterLink :to="`/findings/${f.id}`" class="font-medium hover:underline">
            {{ f.title ?? statusLabel(f.category) }}
          </RouterLink>
          <div v-if="f.rule_id" class="font-mono text-xs text-muted-foreground">
            {{ f.rule_id }}
          </div>
        </TableCell>
        <TableCell v-if="showChange">
          <RouterLink :to="`/changes/${f.change_id}`" class="hover:underline">
            {{ f.repository.full_name }}#{{ f.number ?? '?' }}
          </RouterLink>
        </TableCell>
        <TableCell class="font-mono text-xs">{{ f.path ?? '—' }}</TableCell>
        <TableCell><StatusBadge :value="f.state" /></TableCell>
        <TableCell class="text-muted-foreground">{{ formatTime(f.updated_at) }}</TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
