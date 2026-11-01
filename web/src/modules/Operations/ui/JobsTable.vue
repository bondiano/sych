<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { ACTIVE_STATES, useCancelRun } from '@/modules/Changes'
import { can, useMe } from '@/modules/Session'
import {
  Button,
  ErrorNotice,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

defineProps<{ jobs: Schemas['JobEntry'][] }>()

const { data: me } = useMe()
const cancel = useCancelRun()
</script>

<template>
  <ErrorNotice v-if="cancel.error.value" :error="cancel.error.value" title="Not cancelled" />
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Job</TableHead>
        <TableHead>Pull request</TableHead>
        <TableHead>State</TableHead>
        <TableHead class="text-right">Attempts</TableHead>
        <TableHead>Created</TableHead>
        <TableHead>Error</TableHead>
        <TableHead />
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!jobs.length" :colspan="7">No jobs.</TableEmpty>
      <TableRow v-for="j in jobs" :key="j.id">
        <TableCell>
          <div class="font-mono text-xs">{{ j.kind }}</div>
          <RouterLink :to="`/runs/${j.run_id}`" class="text-xs hover:underline">
            run <StatusBadge :value="j.run_state" />
          </RouterLink>
        </TableCell>
        <TableCell>
          <RouterLink :to="`/changes/${j.change_id}`" class="hover:underline">
            {{ j.repository.full_name }}#{{ j.number ?? '?' }}
          </RouterLink>
        </TableCell>
        <TableCell>
          <StatusBadge :value="j.state" />
          <div v-if="j.state === 'QUEUED'" class="text-xs text-muted-foreground">
            next {{ formatTime(j.next_attempt_at) }}
          </div>
          <div v-else-if="j.lease_until != null" class="text-xs text-muted-foreground">
            leased to {{ formatTime(j.lease_until) }}
          </div>
        </TableCell>
        <TableCell class="text-right tabular-nums"
          >{{ j.attempts }} / {{ j.max_attempts }}</TableCell
        >
        <TableCell class="text-muted-foreground">{{ formatTime(j.created_at) }}</TableCell>
        <TableCell
          class="max-w-xs truncate text-muted-foreground"
          :title="j.run_failure ?? j.last_error ?? ''"
        >
          {{ j.run_failure ?? j.last_error ?? '' }}
        </TableCell>
        <TableCell class="text-right">
          <Button
            v-if="ACTIVE_STATES.includes(j.run_state) && can(me, 'writer', j.repository.id)"
            variant="outline"
            size="sm"
            :disabled="cancel.isPending.value"
            @click="cancel.mutate(j.run_id)"
          >
            Cancel run
          </Button>
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
