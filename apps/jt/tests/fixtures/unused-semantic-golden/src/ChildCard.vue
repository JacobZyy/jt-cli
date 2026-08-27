<template>
  <button type="button" :disabled="loading">
    <slot />
  </button>
</template>

<script setup lang="ts">
defineProps<{ loading: boolean }>()
</script>
