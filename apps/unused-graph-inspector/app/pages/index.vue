<script setup lang="ts">
import type { GraphMeta, GraphPayload } from '#shared/types/graph'

useHead({
  title: 'Unused Graph Inspector',
  meta: [
    { name: 'description', content: 'Explore file dependencies and directed call flows.' },
  ],
})

const { data: meta, error: metaError } = await useFetch<GraphMeta>('/api/meta')
const { data: overview, error: overviewError } = await useFetch<GraphPayload>('/api/overview')

if (metaError.value || overviewError.value || !meta.value || !overview.value) {
  throw createError({
    statusCode: 500,
    statusMessage: metaError.value?.statusMessage
      ?? overviewError.value?.statusMessage
      ?? 'Graph database did not return an overview',
  })
}

const graphMeta = meta.value
const inspector = useInspectorGraph(overview.value)

function updateQuery(value: string) {
  inspector.query.value = value
}

function inspectRelatedNode(nodeId: string) {
  if (inspector.graph.value.mode === 'overview') {
    inspector.selectNode(nodeId)
    return
  }
  void inspector.focus(nodeId)
}
</script>

<template>
  <main class="inspector-shell">
    <GraphToolbar
      :meta="graphMeta"
      :mode="inspector.graph.value.mode"
      :query="inspector.query.value"
      :results="inspector.searchResults.value"
      :depth="inspector.depth.value"
      :loading="inspector.loading.value"
      @update-query="updateQuery"
      @update-depth="inspector.updateDepth"
      @focus="inspector.focus"
      @overview="inspector.showOverview"
    />

    <p v-if="inspector.error.value" class="error-banner" role="alert">
      {{ inspector.error.value }}
    </p>

    <div class="workspace">
      <section class="graph-panel">
        <div class="graph-meta">
          <span>{{ inspector.graph.value.mode === 'overview' ? 'Project area dependencies' : 'Directed call flow' }}</span>
          <span>
            {{ inspector.graph.value.nodes.length }} visible nodes · {{ inspector.graph.value.edges.length }} visible edges
            {{ inspector.graph.value.truncated ? ' · limited' : '' }}
          </span>
        </div>
        <ClientOnly>
          <GraphCanvas
            :graph="inspector.graph.value"
            :selected-id="inspector.selectedId.value"
            @select="inspector.selectNode"
          />
          <template #fallback>
            <div class="graph-loading">Preparing graph renderer…</div>
          </template>
        </ClientOnly>
      </section>

      <NodeDetails
        :graph="inspector.graph.value"
        :node="inspector.selectedNode.value"
        @focus="inspectRelatedNode"
      />
    </div>
  </main>
</template>

<style scoped>
.inspector-shell {
  display: grid;
  grid-template-rows: auto auto minmax(0, 1fr);
  height: 100vh;
  min-height: 640px;
}

.workspace {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 310px;
  min-height: 0;
}

.graph-panel {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  min-width: 0;
}

.graph-meta {
  align-items: center;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
  color: var(--muted);
  display: flex;
  font-size: 11px;
  justify-content: space-between;
  padding: 8px 14px;
}

.graph-meta span:first-child {
  color: var(--text);
  font-weight: 650;
}

.graph-loading {
  align-items: center;
  color: var(--muted);
  display: flex;
  height: 100%;
  justify-content: center;
}

.error-banner {
  background: #fff1f0;
  border-bottom: 1px solid #f7b4ad;
  color: #a12319;
  font-size: 12px;
  margin: 0;
  padding: 8px 16px;
}

@media (max-width: 900px) {
  .inspector-shell {
    height: auto;
  }

  .workspace {
    grid-template-columns: 1fr;
  }

  .graph-panel {
    height: 70vh;
  }
}
</style>
