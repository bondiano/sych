<script setup lang="ts">
import { Button } from './ui/button'

defineProps<{ more: boolean; loading: boolean }>()
defineEmits<{ next: [] }>()
</script>

<template>
  <div v-if="more" class="flex justify-center pt-4">
    <Button variant="outline" size="sm" :disabled="loading" @click="$emit('next')">
      {{ loading ? 'Loading…' : 'Load more' }}
    </Button>
  </div>
</template>
