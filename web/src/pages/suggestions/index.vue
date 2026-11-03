<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { can, RepositorySelect, useMe } from '@/modules/Session'
import { SuggestionCard, useSuggestions } from '@/modules/Suggestions'
import { ErrorNotice, Skeleton } from '@/modules/Ui'

const repository = useQueryParam('repository')
const id = computed(() => toId(repository.value))
const suggestions = useSuggestions(id)
const { data: me } = useMe()
const pending = computed(() => suggestions.data.value?.filter((s) => s.state === 'PENDING') ?? [])
const decided = computed(() => suggestions.data.value?.filter((s) => s.state !== 'PENDING') ?? [])
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Rule suggestions</h1>
      <RepositorySelect v-model="repository" />
    </div>
    <p class="text-sm text-muted-foreground">
      Computed from the feedback of the last 90 days against the repository's current policy, with
      no model. Sych never edits <span class="font-mono">.sych/</span>: apply a suggestion by a pull
      request, then record it here.
    </p>
    <p v-if="id === undefined" class="text-sm text-muted-foreground">Choose a repository.</p>
    <ErrorNotice
      v-else-if="suggestions.error.value"
      :error="suggestions.error.value"
      title="No suggestions to show"
    />
    <Skeleton v-else-if="!suggestions.data.value" class="h-40 w-full" />
    <template v-else>
      <p v-if="!pending.length" class="text-sm text-muted-foreground">Nothing to suggest.</p>
      <SuggestionCard
        v-for="s in pending"
        :key="s.fingerprint"
        :suggestion="s"
        :repository-id="id"
        :can-decide="can(me, 'maintainer', id)"
      />
      <details v-if="decided.length">
        <summary class="cursor-pointer text-sm text-muted-foreground">
          {{ decided.length }} decided
        </summary>
        <div class="mt-3 space-y-3">
          <SuggestionCard
            v-for="s in decided"
            :key="s.fingerprint"
            :suggestion="s"
            :repository-id="id"
            :can-decide="false"
          />
        </div>
      </details>
    </template>
  </div>
</template>
