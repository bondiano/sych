<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { can, useMe } from '@/modules/Session'
import { StatusBadge } from '@/modules/Ui'

import ReconcileForm from './ReconcileForm.vue'

defineProps<{ publications: Schemas['UnresolvedPublication'][] }>()

const { data: me } = useMe()
</script>

<template>
  <p v-if="!publications.length" class="text-sm text-muted-foreground">
    Every publication is settled.
  </p>
  <ul class="space-y-4">
    <li v-for="p in publications" :key="p.id" class="space-y-2 rounded-lg border p-4">
      <div class="flex flex-wrap items-center gap-2 text-sm">
        <StatusBadge :value="p.state" />
        <RouterLink :to="`/changes/${p.change_id}`" class="font-medium hover:underline">
          {{ p.repository.full_name }}#{{ p.number ?? '?' }}
        </RouterLink>
        <span class="font-mono text-xs">{{ p.slot }}</span>
        <span class="text-muted-foreground">
          since {{ formatTime(p.created_at) }} · {{ p.attempts }} attempts
          <template v-if="p.reconcile_deadline != null">
            · reconciler looks until {{ formatTime(p.reconcile_deadline) }}
          </template>
        </span>
      </div>
      <p v-if="p.last_error" class="text-sm text-muted-foreground">{{ p.last_error }}</p>
      <ReconcileForm
        v-if="p.state === 'UNCERTAIN' && can(me, 'writer', p.repository.id)"
        :publication-id="p.id"
      />
    </li>
  </ul>
</template>
