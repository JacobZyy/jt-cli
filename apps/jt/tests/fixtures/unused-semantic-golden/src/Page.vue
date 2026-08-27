<template>
  <ChildCard :loading="loading" @click="handleClick" />
</template>

<script setup lang="ts">
import ChildCard from './ChildCard.vue'

const loading = false

function handleClick(): void {}

function unusedInVue(): void {}
</script>
