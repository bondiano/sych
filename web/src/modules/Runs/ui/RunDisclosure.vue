<script setup lang="ts">
import { ChevronRight } from '@lucide/vue'

defineProps<{ title: string; count?: number }>()
</script>

<template>
  <details class="group rounded-lg border bg-card">
    <summary
      class="flex cursor-pointer list-none items-center gap-2 rounded-lg px-4 py-3 text-sm font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none [&::-webkit-details-marker]:hidden"
    >
      <ChevronRight
        aria-hidden="true"
        class="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90"
      />
      {{ title }}
      <span v-if="count !== undefined" class="font-normal text-muted-foreground tabular-nums">
        {{ count }}
      </span>
    </summary>
    <div class="border-t">
      <slot />
    </div>
  </details>
</template>
