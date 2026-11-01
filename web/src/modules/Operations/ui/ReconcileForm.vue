<script setup lang="ts">
import { computed, ref, watch } from 'vue'

import { idempotencyKey } from '@/modules/Api'
import { Button, ErrorNotice, Label, Textarea } from '@/modules/Ui'

import { useReconcile } from '../queries'

const props = defineProps<{ publicationId: number }>()

const reason = ref('')
const key = ref(idempotencyKey())
const reconcile = useReconcile()
const id = computed(() => `reason-${props.publicationId}`)

// Changed content is a new request; resending the same content replays the first.
watch(reason, () => (key.value = idempotencyKey()))

function send() {
  reconcile.mutate({ id: props.publicationId, reason: reason.value.trim(), key: key.value })
}
</script>

<template>
  <form class="space-y-2" @submit.prevent="send">
    <p class="text-sm">
      It may have reached GitHub already: creating it again can leave a duplicate comment.
    </p>
    <Label :for="id">Why create it again</Label>
    <Textarea :id="id" v-model="reason" required rows="2" />
    <Button
      type="submit"
      variant="outline"
      size="sm"
      :disabled="reconcile.isPending.value || !reason.trim()"
    >
      Create again
    </Button>
    <ErrorNotice v-if="reconcile.error.value" :error="reconcile.error.value" title="Not decided" />
  </form>
</template>
