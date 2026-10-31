<script setup lang="ts">
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { signInWithToken, useSignIns } from '@/modules/Session'
import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Input,
  Label,
  Separator,
} from '@/modules/Ui'

const route = useRoute()
const router = useRouter()
const { data: signIns } = useSignIns()
const token = ref('')
const failure = ref<string>()
const logo = `${import.meta.env.BASE_URL}sych.png`

async function useToken() {
  failure.value = undefined
  try {
    await signInWithToken(token.value)
    token.value = ''
    const next = typeof route.query.next === 'string' ? route.query.next : '/'
    await router.replace(next.startsWith('/') && !next.startsWith('//') ? next : '/')
  } catch (e) {
    failure.value = e instanceof Error ? e.message : 'Sign-in failed'
  }
}
</script>

<template>
  <main class="flex min-h-screen items-center justify-center p-6">
    <Card class="w-full max-w-sm">
      <CardHeader class="items-center text-center">
        <img :src="logo" alt="" class="mx-auto size-12" />
        <CardTitle>Sign in to Sych</CardTitle>
        <CardDescription>Your GitHub account decides what you can see and do.</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <Button v-for="s in signIns" :key="s.connection_id" as-child class="w-full">
          <a :href="s.login_url">Continue with GitHub ({{ s.connection_id }})</a>
        </Button>
        <Separator v-if="signIns?.length" />
        <form class="space-y-2" @submit.prevent="useToken">
          <Label for="token">API client token</Label>
          <Input id="token" v-model="token" type="password" autocomplete="off" required />
          <Button type="submit" variant="outline" class="w-full">Sign in with a token</Button>
        </form>
        <Alert v-if="failure" variant="destructive">
          <AlertDescription>{{ failure }}</AlertDescription>
        </Alert>
      </CardContent>
    </Card>
  </main>
</template>
