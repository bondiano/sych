<script setup lang="ts">
import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import {
  Button,
  ErrorNotice,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

import { ACTIVE_STATES, useCancelRun } from '../queries'

defineProps<{ runs: Schemas['RunStatus'][]; canCancel?: boolean }>()

const cancel = useCancelRun()
</script>

<template>
  <ErrorNotice v-if="cancel.error.value" :error="cancel.error.value" title="Not cancelled" />
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Revision</TableHead>
        <TableHead>State</TableHead>
        <TableHead>Coverage</TableHead>
        <TableHead class="text-right">Unknown obligations</TableHead>
        <TableHead class="text-right">Cost</TableHead>
        <TableHead>Started</TableHead>
        <TableHead>Failure</TableHead>
        <TableHead v-if="canCancel" />
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableRow v-for="r in [...runs].reverse()" :key="r.run_id">
        <TableCell>
          <RouterLink :to="`/runs/${r.run_id}`" class="font-medium hover:underline">
            {{ r.generation }}
          </RouterLink>
          <span v-if="r.parent_run_id" class="text-xs text-muted-foreground"> successor</span>
        </TableCell>
        <TableCell><StatusBadge :value="r.state" /></TableCell>
        <TableCell><StatusBadge :value="r.coverage" /></TableCell>
        <TableCell class="text-right tabular-nums">{{ r.unknown_obligations ?? '—' }}</TableCell>
        <TableCell class="text-right tabular-nums">
          {{ formatMicroUsd(r.usage.cost_known_micro_usd) }}
          <span v-if="r.usage.unknown_cost_attempts" class="text-muted-foreground">
            + {{ r.usage.unknown_cost_attempts }} unknown
          </span>
        </TableCell>
        <TableCell class="text-muted-foreground">{{ formatTime(r.created_at) }}</TableCell>
        <TableCell class="max-w-xs truncate text-muted-foreground" :title="r.failure ?? ''">
          {{ r.failure ?? '' }}
        </TableCell>
        <TableCell v-if="canCancel" class="text-right">
          <Button
            v-if="ACTIVE_STATES.includes(r.state)"
            variant="outline"
            size="sm"
            :disabled="cancel.isPending.value"
            @click="cancel.mutate(r.run_id)"
          >
            Cancel
          </Button>
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
