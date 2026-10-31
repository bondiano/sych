<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { useRouter } from 'vue-router'

import { signOut, useMe } from '@/modules/Session'
import { Button, Separator } from '@/modules/Ui'

const router = useRouter()
const queries = useQueryClient()
const { data: me } = useMe()
const logo = `${import.meta.env.BASE_URL}sych.png`

async function leave() {
  await signOut(queries)
  await router.push('/login')
}
</script>

<template>
  <div class="min-h-screen">
    <header class="flex items-center gap-6 px-6 py-3">
      <RouterLink to="/" class="flex items-center gap-2 font-semibold">
        <img :src="logo" alt="" class="size-7" />
        Sych
      </RouterLink>
      <nav class="flex gap-4 text-sm" aria-label="Main">
        <RouterLink to="/" class="text-muted-foreground" active-class="text-foreground">
          Overview
        </RouterLink>
      </nav>
      <div class="ml-auto flex items-center gap-3 text-sm">
        <span v-if="me" class="text-muted-foreground">{{ me.actor }}</span>
        <Button variant="ghost" size="sm" @click="leave">Sign out</Button>
      </div>
    </header>
    <Separator />
    <main class="mx-auto max-w-6xl px-6 py-8">
      <slot />
    </main>
  </div>
</template>
