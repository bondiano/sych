<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

defineProps<{ change: Schemas['ChangeDetail'] }>()
</script>

<template>
  <div class="grid gap-6 lg:grid-cols-2">
    <section>
      <h3 class="mb-2 font-medium">Publications</h3>
      <p v-if="!change.publications.length" class="text-sm text-muted-foreground">None.</p>
      <ul class="space-y-2 text-sm">
        <li v-for="p in change.publications" :key="p.id" class="flex flex-wrap items-center gap-2">
          <span class="lowercase">{{ p.kind }}</span>
          <StatusBadge :value="p.state" />
          <StatusBadge v-if="p.relevance === 'STALE'" :value="p.relevance" />
          <span class="text-muted-foreground"
            >rev {{ p.generation }} · {{ formatTime(p.created_at) }}</span
          >
          <span v-if="p.last_error" class="w-full text-destructive">{{ p.last_error }}</span>
        </li>
      </ul>
    </section>
    <section>
      <h3 class="mb-2 font-medium">Events</h3>
      <p v-if="!change.events.length" class="text-sm text-muted-foreground">None.</p>
      <ol class="space-y-1 text-sm">
        <li v-for="(e, i) in change.events" :key="i" class="flex gap-2">
          <span class="shrink-0 text-muted-foreground">{{ formatTime(e.at) }}</span>
          <span class="font-mono text-xs leading-5">{{ e.kind }}</span>
        </li>
      </ol>
    </section>
  </div>
</template>
