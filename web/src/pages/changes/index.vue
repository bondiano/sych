<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { ChangesTable, useChanges } from '@/modules/Changes'
import { RepositorySelect } from '@/modules/Session'
import {
  ErrorNotice,
  LoadMore,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/modules/Ui'

const repository = useQueryParam('repository')
const state = useQueryParam('state')
const changes = useChanges(
  computed(() => ({
    repository_id: repository.value ? Number(repository.value) : undefined,
    state: state.value,
  })),
)
const rows = computed(() => changes.data.value?.pages.flatMap((p) => p.items) ?? [])
const ANY = 'any'
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Pull requests</h1>
      <RepositorySelect v-model="repository" />
      <Select
        :model-value="state ?? ANY"
        @update:model-value="state = $event === ANY ? undefined : String($event)"
      >
        <SelectTrigger class="w-36" aria-label="State"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem :value="ANY">Any state</SelectItem>
          <SelectItem v-for="s in ['OPEN', 'DRAFT', 'MERGED', 'CLOSED']" :key="s" :value="s">
            {{ s.toLowerCase() }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
    <ErrorNotice v-if="changes.error.value" :error="changes.error.value" />
    <ChangesTable :changes="rows" />
    <LoadMore
      :more="changes.hasNextPage.value"
      :loading="changes.isFetchingNextPage.value"
      @next="changes.fetchNextPage()"
    />
  </div>
</template>
