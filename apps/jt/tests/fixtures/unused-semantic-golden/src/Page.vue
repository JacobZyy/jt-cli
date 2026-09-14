<template>
  <ChildCard :loading="loading" @click="handleClick" />
</template>

<script setup lang="ts">
import ChildCard from './ChildCard.vue'

const loading = false

function handleClick(): void {}

function unusedInVue(): void {}

function readNestedLoading(): boolean {
  const loading = true
  return loading
}

readNestedLoading()

const deadOnly = false

function deadRead(): boolean {
  return deadOnly
}
</script>
