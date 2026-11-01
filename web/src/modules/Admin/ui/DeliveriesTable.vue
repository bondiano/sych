<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import {
  statusLabel,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

defineProps<{ deliveries: Schemas['Delivery'][] }>()
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Received</TableHead>
        <TableHead>Connection</TableHead>
        <TableHead>Event</TableHead>
        <TableHead>Decision</TableHead>
        <TableHead>Delivery</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!deliveries.length" :colspan="5">No deliveries.</TableEmpty>
      <TableRow v-for="d in deliveries" :key="d.id">
        <TableCell class="text-muted-foreground">{{ formatTime(d.received_at) }}</TableCell>
        <TableCell class="font-mono text-xs">{{ d.connection_id }}</TableCell>
        <TableCell
          >{{ d.event }}<template v-if="d.action">.{{ d.action }}</template></TableCell
        >
        <TableCell>{{ statusLabel(d.decision) }}</TableCell>
        <TableCell class="font-mono text-xs">{{ d.delivery_id }}</TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
