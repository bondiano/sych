<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { useMe } from '@/modules/Session'
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

import { targetLink } from '../target'

defineProps<{ events: Schemas['AuditEvent'][] }>()

const { data: me } = useMe()
const repositoryName = (id: number | null | undefined) =>
  id == null
    ? '—'
    : (me.value?.repositories.find((g) => g.repository.id === id)?.repository.full_name ?? `#${id}`)
const details = (metadata: unknown) =>
  metadata == null || (typeof metadata === 'object' && !Object.keys(metadata).length)
    ? ''
    : JSON.stringify(metadata)
</script>

<template>
  <Table>
    <TableHeader>
      <TableRow>
        <TableHead>When</TableHead>
        <TableHead>Who</TableHead>
        <TableHead>Action</TableHead>
        <TableHead>Repository</TableHead>
        <TableHead>Target</TableHead>
        <TableHead>Details</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableEmpty v-if="!events.length" :colspan="6">Nothing recorded.</TableEmpty>
      <TableRow v-for="e in events" :key="e.id">
        <TableCell class="text-muted-foreground">{{ formatTime(e.at) }}</TableCell>
        <TableCell>{{ e.actor }}</TableCell>
        <TableCell>{{ statusLabel(e.action) }}</TableCell>
        <TableCell>{{ repositoryName(e.repository_id) }}</TableCell>
        <TableCell class="font-mono text-xs">
          <RouterLink
            v-if="targetLink(e.target)"
            :to="targetLink(e.target)!"
            class="hover:underline"
          >
            {{ e.target }}
          </RouterLink>
          <template v-else>{{ e.target }}</template>
        </TableCell>
        <TableCell class="max-w-sm truncate font-mono text-xs" :title="details(e.metadata)">
          {{ details(e.metadata) }}
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
