<script setup lang="ts">
import { useMe } from '@/modules/Session'
import {
  Badge,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Skeleton,
} from '@/modules/Ui'

const { data: me, isPending } = useMe()
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>Repositories</CardTitle>
      <CardDescription>What you can read here, and your role on each.</CardDescription>
    </CardHeader>
    <CardContent>
      <Skeleton v-if="isPending" class="h-6 w-64" />
      <p v-else-if="!me?.repositories.length" class="text-sm text-muted-foreground">
        No repositories: your GitHub account reaches none that Sych reviews.
      </p>
      <ul v-else class="space-y-2">
        <li v-for="g in me.repositories" :key="g.repository.id" class="flex items-center gap-3">
          <span class="font-mono text-sm">{{ g.repository.full_name }}</span>
          <Badge variant="secondary">{{ g.role }}</Badge>
        </li>
      </ul>
    </CardContent>
  </Card>
</template>
