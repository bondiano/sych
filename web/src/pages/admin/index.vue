<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import {
  ConnectionCard,
  DECISIONS,
  DeliveriesTable,
  useConnections,
  useDeliveries,
  useEffectiveConfig,
} from '@/modules/Admin'
import {
  ErrorNotice,
  LoadMore,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  statusLabel,
} from '@/modules/Ui'

const connection = useQueryParam('connection')
const decision = useQueryParam('decision')
const connections = useConnections()
const deliveries = useDeliveries(
  computed(() => ({ connection_id: connection.value, decision: decision.value })),
)
const rows = computed(() => deliveries.data.value?.pages.flatMap((p) => p.items) ?? [])
const config = useEffectiveConfig()

// An empty choice clears the filter; a select item cannot carry an empty value.
const ANY = 'any'
const pick = (value: unknown) => (value === ANY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <div class="space-y-8">
    <header class="space-y-1">
      <h1 class="text-xl font-semibold">Administration</h1>
      <p class="text-sm text-muted-foreground">
        Read only: connections, models, budgets and clients change in the server's YAML, policy by a
        pull request to <span class="font-mono">.sych/</span>.
      </p>
    </header>
    <section class="space-y-4">
      <h2 class="text-lg font-medium">Connections</h2>
      <ErrorNotice v-if="connections.error.value" :error="connections.error.value" />
      <Skeleton v-else-if="!connections.data.value" class="h-32 w-full" />
      <p v-else-if="!connections.data.value.length" class="text-sm text-muted-foreground">
        No GitHub connection is configured.
      </p>
      <ConnectionCard v-for="c in connections.data.value ?? []" :key="c.id" :connection="c" />
    </section>
    <section>
      <div class="mb-3 flex flex-wrap items-center gap-3">
        <h2 class="mr-auto text-lg font-medium">Webhook deliveries</h2>
        <Select :model-value="connection ?? ANY" @update:model-value="connection = pick($event)">
          <SelectTrigger class="w-48" aria-label="Connection"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem :value="ANY">Every connection</SelectItem>
            <SelectItem v-for="c in connections.data.value ?? []" :key="c.id" :value="c.id">
              {{ c.id }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Select :model-value="decision ?? ANY" @update:model-value="decision = pick($event)">
          <SelectTrigger class="w-56" aria-label="Decision"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem :value="ANY">Every decision</SelectItem>
            <SelectItem v-for="d in DECISIONS" :key="d" :value="d">{{ statusLabel(d) }}</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <ErrorNotice v-if="deliveries.error.value" :error="deliveries.error.value" />
      <DeliveriesTable :deliveries="rows" />
      <LoadMore
        :more="deliveries.hasNextPage.value"
        :loading="deliveries.isFetchingNextPage.value"
        @next="deliveries.fetchNextPage()"
      />
    </section>
    <section>
      <h2 class="mb-3 text-lg font-medium">Effective config</h2>
      <ErrorNotice v-if="config.error.value" :error="config.error.value" />
      <details v-else-if="config.data.value" class="rounded-lg border p-4">
        <summary class="cursor-pointer text-sm">
          As <span class="font-mono">sych config validate</span> prints it: secrets by their env
          names only
        </summary>
        <pre class="mt-3 overflow-x-auto text-xs">{{
          JSON.stringify(config.data.value, null, 2)
        }}</pre>
      </details>
    </section>
  </div>
</template>
