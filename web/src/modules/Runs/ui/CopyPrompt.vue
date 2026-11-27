<script setup lang="ts">
import { Check, Copy } from '@lucide/vue'
import { computed, ref } from 'vue'

import { Button } from '@/modules/Ui'

import { strings, type Lang } from '../i18n'

const props = defineProps<{ text: string; label: string; language?: Lang }>()

const t = computed(() => strings(props.language))
const statusText = computed(() => ({
  idle: '',
  copied: t.value.copied,
  refused: t.value.copyRefused,
}))

const state = ref<'idle' | 'copied' | 'refused'>('idle')

async function copy() {
  try {
    await navigator.clipboard.writeText(props.text)
    state.value = 'copied'
    setTimeout(() => (state.value = 'idle'), 2000)
  } catch {
    // Without clipboard access (e.g. restricted file:// context) the text stays selectable below.
    state.value = 'refused'
  }
}
</script>

<template>
  <div class="flex flex-col items-start gap-2 print:hidden">
    <Button variant="outline" size="sm" type="button" @click="copy">
      <Check v-if="state === 'copied'" aria-hidden="true" />
      <Copy v-else aria-hidden="true" />
      {{ state === 'copied' ? t.copied : label }}
    </Button>
    <span class="sr-only" role="status">{{ statusText[state] }}</span>
    <pre
      v-if="state === 'refused'"
      class="max-h-64 max-w-full overflow-auto rounded-md border bg-muted p-3 font-mono text-xs whitespace-pre-wrap"
      >{{ text }}</pre>
  </div>
</template>
