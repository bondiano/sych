<script setup lang="ts">
import { computed, ref, watch } from 'vue'

import { idempotencyKey } from '@/modules/Api'
import { Button, ErrorNotice, Input, Label, Textarea } from '@/modules/Ui'

import { useCreateWaiver } from '../queries'

const props = defineProps<{ changeId: number; ruleId: string; path?: string | null }>()

const paths = ref(props.path ?? '')
const owner = ref('')
const reason = ref('')
// `datetime-local` is the browser's local time; the request carries it as UTC.
const expires = ref('')
const until = ref('')
const key = ref(idempotencyKey())
const waiver = useCreateWaiver()
const recorded = ref<number>()

const list = computed(() =>
  paths.value
    .split('\n')
    .map((p) => p.trim())
    .filter(Boolean),
)
const ready = computed(
  () =>
    list.value.length > 0 &&
    owner.value.trim() !== '' &&
    reason.value.trim() !== '' &&
    (expires.value !== '' || until.value.trim() !== '') &&
    // A waiver that has already expired would accept nothing.
    (expires.value === '' || new Date(expires.value).getTime() > Date.now()),
)

// Changed content is a new request; resending the same content replays the first.
watch([paths, owner, reason, expires, until], () => (key.value = idempotencyKey()))

function send() {
  const body = {
    change_id: props.changeId,
    rule_id: props.ruleId,
    paths: list.value,
    owner: owner.value.trim(),
    reason: reason.value.trim(),
    expires_at: expires.value ? new Date(expires.value).toISOString() : null,
    until: until.value.trim() || null,
  }
  recorded.value = undefined
  waiver.mutate(
    { body, key: key.value },
    {
      onSuccess: (data) => {
        recorded.value = (data as { waiver_id?: number } | undefined)?.waiver_id
        reason.value = ''
        key.value = idempotencyKey()
      },
    },
  )
}
</script>

<template>
  <form class="space-y-3" @submit.prevent="send">
    <p class="text-sm text-muted-foreground">
      Accepts <span class="font-mono">{{ ruleId }}</span> on these paths: the next review lowers its
      risk decision, the rule's status stays as assessed.
    </p>
    <div class="space-y-1">
      <Label for="waiver-paths">Paths, one per line</Label>
      <Textarea id="waiver-paths" v-model="paths" required rows="2" class="font-mono text-xs" />
    </div>
    <div class="space-y-1">
      <Label for="waiver-owner">Owner</Label>
      <Input id="waiver-owner" v-model="owner" required />
    </div>
    <div class="space-y-1">
      <Label for="waiver-reason">Why</Label>
      <Textarea id="waiver-reason" v-model="reason" required rows="3" />
    </div>
    <div class="space-y-1">
      <Label for="waiver-expires">Expires</Label>
      <Input id="waiver-expires" v-model="expires" type="datetime-local" />
    </div>
    <div class="space-y-1">
      <Label for="waiver-until">Or until</Label>
      <Input id="waiver-until" v-model="until" placeholder="the migration lands" />
    </div>
    <Button type="submit" :disabled="waiver.isPending.value || !ready">Record waiver</Button>
    <p v-if="recorded !== undefined" class="text-sm">Waiver {{ recorded }} recorded.</p>
    <ErrorNotice v-if="waiver.error.value" :error="waiver.error.value" title="Not recorded" />
  </form>
</template>
