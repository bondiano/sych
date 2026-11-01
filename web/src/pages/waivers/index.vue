<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { RepositorySelect } from '@/modules/Session'
import {
  ErrorNotice,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
} from '@/modules/Ui'
import { useWaivers, WaiversTable } from '@/modules/Waivers'

const repository = useQueryParam('repository')
const state = useQueryParam('state')
const waivers = useWaivers(
  computed(() => ({ repository_id: toId(repository.value), state: state.value })),
)

// An empty choice clears the filter; a select item cannot carry an empty value.
const ANY = 'any'
const STATES = ['active', 'expired', 'revoked']
const pick = (value: unknown) => (value === ANY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Waivers</h1>
      <RepositorySelect v-model="repository" />
      <Select :model-value="state ?? ANY" @update:model-value="state = pick($event)">
        <SelectTrigger class="w-40" aria-label="State"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem :value="ANY">Any state</SelectItem>
          <SelectItem v-for="s in STATES" :key="s" :value="s">{{ s }}</SelectItem>
        </SelectContent>
      </Select>
    </div>
    <p class="text-sm text-muted-foreground">
      A waiver is recorded from a rule's finding; one that expires within a week shows as expiring.
    </p>
    <ErrorNotice v-if="waivers.error.value" :error="waivers.error.value" />
    <Skeleton v-else-if="!waivers.data.value" class="h-40 w-full" />
    <WaiversTable v-else :waivers="waivers.data.value" />
  </div>
</template>
