<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { ACTIONS, AuditTable, useAudit } from '@/modules/Audit'
import {
  ErrorNotice,
  LoadMore,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  statusLabel,
} from '@/modules/Ui'

const action = useQueryParam('action')
const audit = useAudit(computed(() => ({ action: action.value })))
const rows = computed(() => audit.data.value?.pages.flatMap((p) => p.items) ?? [])

// An empty choice clears the filter; a select item cannot carry an empty value.
const ANY = 'any'
const pick = (value: unknown) => (value === ANY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Audit</h1>
      <Select :model-value="action ?? ANY" @update:model-value="action = pick($event)">
        <SelectTrigger class="w-64" aria-label="Action"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem :value="ANY">Every action</SelectItem>
          <SelectItem v-for="a in ACTIONS" :key="a" :value="a">{{ statusLabel(a) }}</SelectItem>
        </SelectContent>
      </Select>
    </div>
    <p class="text-sm text-muted-foreground">
      Actions on the repositories you maintain; sign-ins only for an operator of every repository.
    </p>
    <ErrorNotice v-if="audit.error.value" :error="audit.error.value" />
    <AuditTable :events="rows" />
    <LoadMore
      :more="audit.hasNextPage.value"
      :loading="audit.isFetchingNextPage.value"
      @next="audit.fetchNextPage()"
    />
  </div>
</template>
