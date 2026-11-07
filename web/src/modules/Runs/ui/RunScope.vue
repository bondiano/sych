<script setup lang="ts">
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

defineProps<{ scope: Schemas['ScopeManifest'] }>()
</script>

<template>
  <ul class="divide-y">
    <li v-for="u in scope.units" :key="u.id" class="space-y-1 p-4 text-sm">
      <p class="font-medium">{{ u.title }}</p>
      <ul class="font-mono text-xs text-muted-foreground">
        <li v-for="p in u.paths" :key="p" class="break-all">{{ p }}</li>
      </ul>
      <p v-if="u.risk_hints?.length" class="flex flex-wrap gap-1">
        <StatusBadge
          v-for="h in u.risk_hints"
          :key="`${h.code}:${h.path}`"
          :value="h.code"
          :title="h.path"
        />
      </p>
    </li>
  </ul>
</template>
