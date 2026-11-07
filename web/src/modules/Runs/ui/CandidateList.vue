<script setup lang="ts">
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

import type { CodeLinks } from '../report'
import CodePlace from './CodePlace.vue'
import ProseText from './ProseText.vue'

defineProps<{ candidates: Schemas['Candidate'][]; links?: CodeLinks }>()
</script>

<template>
  <ul class="divide-y">
    <li v-for="c in candidates" :key="c.id" class="max-w-prose space-y-1 p-4 text-sm">
      <p><StatusBadge :value="c.proposed_severity" /> <ProseText :text="c.claim" /></p>
      <ul>
        <li v-for="a in c.anchors" :key="`${a.side}:${a.path}:${a.start_line}-${a.end_line}`">
          <CodePlace :code="a" :links="links" />
        </li>
      </ul>
      <p v-for="l in c.limitations" :key="l" class="text-muted-foreground">
        <ProseText :text="l" />
      </p>
    </li>
  </ul>
</template>
