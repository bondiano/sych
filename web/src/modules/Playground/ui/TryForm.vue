<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'

import { idempotencyKey } from '@/modules/Api'
import {
  Button,
  ErrorNotice,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/modules/Ui'

import { useTry } from '../queries'

const props = defineProps<{ changeId: number }>()

const router = useRouter()
const profile = ref<'default' | 'deep'>('default')
const key = ref(idempotencyKey())
const attempt = useTry(computed(() => props.changeId))

// Another profile or pull request is a new try; resending the same one replays the first.
watch([profile, () => props.changeId], () => (key.value = idempotencyKey()))

function send() {
  const body = { profile: profile.value === 'deep' ? 'deep' : null }
  attempt.mutate(
    { body, key: key.value },
    {
      onSuccess: (accepted) => {
        key.value = idempotencyKey()
        if (accepted.run_id) void router.push(`/runs/${accepted.run_id}`)
      },
    },
  )
}
</script>

<template>
  <form class="flex flex-wrap items-center gap-3" @submit.prevent="send">
    <Select v-model="profile">
      <SelectTrigger class="w-36" aria-label="Playground profile"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value="default">Default</SelectItem>
        <SelectItem value="deep">Deep</SelectItem>
      </SelectContent>
    </Select>
    <Button type="submit" variant="outline" :disabled="attempt.isPending.value">
      Try in the playground
    </Button>
    <span class="text-xs text-muted-foreground">
      Costs a review; publishes nothing and stays out of findings and analytics.
    </span>
    <ErrorNotice
      v-if="attempt.error.value"
      :error="attempt.error.value"
      title="Not started"
      class="basis-full"
    />
  </form>
</template>
