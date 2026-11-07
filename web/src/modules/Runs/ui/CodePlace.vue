<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'

import { codeLink, place, type CodeLinks } from '../report'

const props = defineProps<{ code: Schemas['CodeRef']; links?: CodeLinks }>()
const href = computed(() => codeLink(props.code, props.links))
</script>

<template>
  <a
    v-if="href"
    :href="href"
    target="_blank"
    rel="noopener noreferrer"
    class="font-mono text-xs break-all underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
    >{{ place(code) }}</a
  >
  <span v-else class="font-mono text-xs break-all">{{ place(code) }}</span>
</template>
