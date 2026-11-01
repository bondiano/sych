<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'

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
} from '@/modules/Ui'

import { useRerun } from '../queries'

const props = defineProps<{ changeId: number }>()

const router = useRouter()
const profile = ref<'default' | 'deep'>('default')
const publish = ref(false)
const key = ref(idempotencyKey())
const rerun = useRerun(computed(() => props.changeId))

// Other options are a new request; resending the same ones replays the first.
watch([profile, publish], () => (key.value = idempotencyKey()))

function send() {
  const body = { profile: profile.value === 'deep' ? 'deep' : null, publish: publish.value }
  rerun.mutate(
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
      <SelectTrigger class="w-36" aria-label="Profile"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value="default">Default</SelectItem>
        <SelectItem value="deep">Deep</SelectItem>
      </SelectContent>
    </Select>
    <Label class="font-normal">
      <input v-model="publish" type="checkbox" class="size-4" />
      Publish to the pull request
    </Label>
    <Button type="submit" :disabled="rerun.isPending.value">Rerun</Button>
    <ErrorNotice
      v-if="rerun.error.value"
      :error="rerun.error.value"
      title="Not started"
      class="basis-full"
    />
  </form>
</template>
