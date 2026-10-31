<script setup lang="ts">
import { Alert, AlertDescription, AlertTitle } from './ui/alert'

defineProps<{ error: unknown; title?: string }>()
</script>

<template>
  <Alert variant="destructive">
    <AlertTitle>{{ title ?? 'Could not load this' }}</AlertTitle>
    <AlertDescription>{{
      error instanceof Error ? error.message : String(error)
    }}</AlertDescription>
  </Alert>
</template>
