<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { computed } from 'vue'

import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
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

import { useRevokeWaiver } from '../queries'
import { waiverState } from '../state'

defineProps<{ waivers: Schemas['WaiverEntry'][] }>()

const { data: me } = useMe()
const revoke = useRevokeWaiver()
// Ticks, so an open page moves a waiver to expiring and expired as time passes.
const clock = useNow({ scheduler: (tick) => useIntervalFn(tick, 60_000) })
const now = computed(() => clock.value.getTime() / 1000)
</script>

<template>
  <ErrorNotice v-if="revoke.error.value" :error="revoke.error.value" title="Not revoked" />
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>Rule</TableHead>
        <TableHead>Paths</TableHead>
        <TableHead>Reason</TableHead>
        <TableHead>Owner</TableHead>
        <TableHead>Ends</TableHead>
        <TableHead>State</TableHead>
        <TableHead />
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!waivers.length" :colspan="7">No waivers.</TableEmpty>
      <TableRow v-for="w in waivers" :key="w.id">
        <TableCell>
          <div class="font-mono text-xs">{{ w.rule_id }}</div>
          <div class="text-xs text-muted-foreground">{{ w.repository.full_name }}</div>
        </TableCell>
        <TableCell class="font-mono text-xs">
          <div v-for="p in w.paths" :key="p">{{ p }}</div>
        </TableCell>
        <TableCell class="max-w-xs whitespace-normal">{{ w.reason }}</TableCell>
        <TableCell>
          {{ w.owner }}
          <div class="text-xs text-muted-foreground">by {{ w.author }}</div>
        </TableCell>
        <TableCell class="whitespace-normal">
          <div v-if="w.expires_at != null">{{ formatTime(w.expires_at) }}</div>
          <div v-if="w.until" class="text-muted-foreground">until {{ w.until }}</div>
        </TableCell>
        <TableCell>
          <StatusBadge :value="waiverState(w, now)" />
          <div v-if="w.revoked_at != null" class="text-xs text-muted-foreground">
            {{ formatTime(w.revoked_at) }}
          </div>
        </TableCell>
        <TableCell class="text-right">
          <Button
            v-if="w.revoked_at == null && can(me, 'maintainer', w.repository.id)"
            variant="outline"
            size="sm"
            :disabled="revoke.isPending.value"
            @click="revoke.mutate(w.id)"
          >
            Revoke
          </Button>
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
