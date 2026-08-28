<script setup lang="ts">
import type { GraphMeta, GraphSearchResult } from '#shared/types/graph'

defineProps<{
  meta: GraphMeta
  mode: 'overview' | 'call-flow'
  query: string
  results: GraphSearchResult[]
  depth: number
  loading: boolean
}>()

const emit = defineEmits<{
  focus: [id: string]
  overview: []
  updateDepth: [value: number]
  updateQuery: [value: string]
}>()

function onInput(event: Event) {
  emit('updateQuery', (event.target as HTMLInputElement).value)
}

function onDepth(event: Event) {
  emit('updateDepth', Number((event.target as HTMLSelectElement).value))
}
</script>

<template>
  <header class="toolbar">
    <div class="brand">
      <span class="mark">UG</span>
      <div>
        <strong>Unused Graph Inspector</strong>
        <span>{{ meta.nodes }} nodes · {{ meta.edges }} edges</span>
      </div>
    </div>

    <div class="search-wrap">
      <input
        class="search"
        type="search"
        :value="query"
        autocomplete="off"
        placeholder="Search symbol, FQN, or path"
        aria-label="Search graph symbols"
        @input="onInput"
      >
      <ul v-if="query.trim() && results.length" class="search-results">
        <li v-for="result in results" :key="result.id">
          <button type="button" @click="emit('focus', result.id)">
            <span>{{ result.name }}</span>
            <small>{{ result.kind }} · {{ result.path }}:{{ result.line }}</small>
          </button>
        </li>
      </ul>
    </div>

    <nav class="view-controls" aria-label="Graph view controls">
      <button
        type="button"
        class="view-button"
        :class="{ active: mode === 'overview' }"
        @click="emit('overview')"
      >
        Overview
      </button>
      <span class="mode-label" :class="{ active: mode === 'call-flow' }">Call Flow</span>
      <label class="depth">
        Depth
        <select :value="depth" :disabled="mode !== 'call-flow' || loading" @change="onDepth">
          <option v-for="value in 4" :key="value" :value="value">{{ value }}</option>
        </select>
      </label>
    </nav>
  </header>
</template>

<style scoped>
.toolbar {
  align-items: center;
  background: color-mix(in srgb, var(--panel) 92%, transparent);
  border-bottom: 1px solid var(--line);
  display: grid;
  gap: 18px;
  grid-template-columns: minmax(250px, auto) minmax(260px, 1fr) auto;
  padding: 13px 18px;
  position: relative;
  z-index: 4;
}

.brand {
  align-items: center;
  display: flex;
  gap: 10px;
}

.brand div {
  display: grid;
  gap: 2px;
}

.brand span {
  color: var(--muted);
  font-size: 12px;
}

.mark {
  align-items: center;
  background: var(--accent);
  border-radius: 8px;
  color: white !important;
  display: flex;
  font-size: 11px !important;
  font-weight: 800;
  height: 34px;
  justify-content: center;
  letter-spacing: .08em;
  width: 34px;
}

.search-wrap {
  position: relative;
}

.search {
  background: var(--surface);
  border: 1px solid var(--line-strong);
  border-radius: 9px;
  color: var(--text);
  font: inherit;
  outline: none;
  padding: 9px 12px;
  width: 100%;
}

.search:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 14%, transparent);
}

.search-results {
  background: var(--panel);
  border: 1px solid var(--line-strong);
  border-radius: 9px;
  box-shadow: var(--shadow);
  left: 0;
  list-style: none;
  margin: 6px 0 0;
  max-height: 360px;
  overflow: auto;
  padding: 5px;
  position: absolute;
  right: 0;
}

.search-results button {
  background: transparent;
  border: 0;
  border-radius: 6px;
  color: var(--text);
  cursor: pointer;
  display: grid;
  gap: 2px;
  padding: 8px 9px;
  text-align: left;
  width: 100%;
}

.search-results button:hover,
.search-results button:focus-visible {
  background: var(--surface-raised);
}

.search-results small {
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.view-controls {
  align-items: center;
  display: flex;
  gap: 7px;
}

.view-button,
.mode-label {
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--muted);
  font-size: 12px;
  padding: 7px 9px;
}

.view-button {
  background: var(--surface);
  cursor: pointer;
}

.view-button.active,
.mode-label.active {
  background: var(--accent-soft);
  border-color: color-mix(in srgb, var(--accent) 35%, var(--line));
  color: var(--accent-strong);
}

.depth {
  align-items: center;
  color: var(--muted);
  display: flex;
  font-size: 12px;
  gap: 6px;
}

.depth select {
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--text);
  padding: 6px;
}

@media (max-width: 900px) {
  .toolbar {
    grid-template-columns: 1fr;
  }
}
</style>
