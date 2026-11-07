<script setup lang="ts">
import { Check, Copy } from '@lucide/vue'
import { ref } from 'vue'

import { Button } from '@/modules/Ui'

const props = defineProps<{ text: string; label: string }>()

const state = ref<'idle' | 'copied' | 'refused'>('idle')

async function copy() {
  try {
    await navigator.clipboard.writeText(props.text)
    state.value = 'copied'
    setTimeout(() => (state.value = 'idle'), 2000)
  } catch {
    // Without clipboard access the text is still there to select by hand.
    state.value = 'refused'
  }
}
</script>

<template>
  <div class="flex flex-col items-start gap-2">
    <Button variant="outline" size="sm" type="button" @click="copy">
      <Check v-if="state === 'copied'" aria-hidden="true" />
      <Copy v-else aria-hidden="true" />
      {{ label }}
    </Button>
    <span class="sr-only" role="status">{{ state === 'copied' ? 'Copied' : '' }}</span>
    <pre
      v-if="state === 'refused'"
      class="max-h-64 overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap"
      >{{ text }}</pre>
  </div>
</template>
