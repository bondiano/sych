<script setup lang="ts">
import { computed, ref, watch } from 'vue'

import { idempotencyKey } from '@/modules/Api'

import {
  Button,
  ErrorNotice,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  statusLabel,
  Textarea,
} from '@/modules/Ui'

import { JUDGEMENTS, useFeedback, type Judgement } from '../queries'

const props = defineProps<{ findingId: number }>()

const judgement = ref<Judgement>('USEFUL')
const rationale = ref('')
const key = ref(idempotencyKey())
const feedback = useFeedback(computed(() => props.findingId))

// Changed content is a new request; resending the same content replays the first.
watch([judgement, rationale], () => (key.value = idempotencyKey()))

function send() {
  const body = { judgement: judgement.value, scope: 'finding', rationale: rationale.value.trim() }
  feedback.mutate(
    { body, key: key.value },
    {
      onSuccess: () => {
        rationale.value = ''
        key.value = idempotencyKey()
      },
    },
  )
}
</script>

<template>
  <form class="space-y-3" @submit.prevent="send">
    <div class="space-y-1">
      <Label for="judgement">Judgement</Label>
      <Select v-model="judgement">
        <SelectTrigger id="judgement" class="w-56"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem v-for="j in JUDGEMENTS" :key="j" :value="j">
            {{ statusLabel(j) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
    <div class="space-y-1">
      <Label for="rationale">Why</Label>
      <Textarea id="rationale" v-model="rationale" required rows="3" />
    </div>
    <p v-if="judgement === 'FALSE_POSITIVE' || judgement === 'WONT_FIX'" class="text-sm">
      This dismisses the finding; later revisions keep it dismissed.
    </p>
    <Button type="submit" :disabled="feedback.isPending.value || !rationale.trim()">
      Record feedback
    </Button>
    <ErrorNotice v-if="feedback.error.value" :error="feedback.error.value" title="Not recorded" />
  </form>
</template>
