<script setup lang="ts">
import type { GraphEdge, GraphNode, GraphPayload } from '#shared/types/graph'

const props = defineProps<{
  graph: GraphPayload
  node: GraphNode | null
}>()

const emit = defineEmits<{
  focus: [id: string]
}>()

const incoming = computed(() => relatedEdges('incoming'))
const outgoing = computed(() => relatedEdges('outgoing'))
const callSites = computed(() => [...incoming.value, ...outgoing.value]
  .flatMap(edge => edge.sites.map(site => ({ edge, site })))
  .slice(0, 30))

function relatedEdges(direction: 'incoming' | 'outgoing'): GraphEdge[] {
  if (!props.node) return []
  const key = direction === 'incoming' ? 'target' : 'source'
  return props.graph.edges.filter(edge => edge[key] === props.node!.id)
}

function otherNode(edge: GraphEdge, direction: 'incoming' | 'outgoing') {
  const id = direction === 'incoming' ? edge.source : edge.target
  return props.graph.nodes.find(node => node.id === id)
}
</script>

<template>
  <aside class="details">
    <div v-if="!node" class="empty">
      <strong>Select a node</strong>
      <span>Inspect its location and visible relationships.</span>
    </div>

    <template v-else>
      <header class="node-header">
        <span class="kind">{{ node.kind }}</span>
        <h2>{{ node.name }}</h2>
        <p>{{ node.qualifiedName }}</p>
      </header>

      <dl class="facts">
        <div>
          <dt>Location</dt>
          <dd>{{ node.path }}:{{ node.line }}:{{ node.column }}</dd>
        </div>
        <div>
          <dt>Language</dt>
          <dd>{{ node.language || 'unknown' }}</dd>
        </div>
        <div>
          <dt>Degree</dt>
          <dd>{{ node.degree }}</dd>
        </div>
      </dl>

      <section class="relation-section">
        <h3>Incoming <span>{{ incoming.length }}</span></h3>
        <p v-if="!incoming.length" class="empty-line">None in current view</p>
        <button
          v-for="edge in incoming"
          :key="edge.id"
          class="relation"
          type="button"
          @click="otherNode(edge, 'incoming') && emit('focus', edge.source)"
        >
          <span>{{ otherNode(edge, 'incoming')?.name ?? edge.source }}</span>
          <small>{{ edge.kind }} · {{ edge.count }}×</small>
        </button>
      </section>

      <section class="relation-section">
        <h3>Outgoing <span>{{ outgoing.length }}</span></h3>
        <p v-if="!outgoing.length" class="empty-line">None in current view</p>
        <button
          v-for="edge in outgoing"
          :key="edge.id"
          class="relation"
          type="button"
          @click="otherNode(edge, 'outgoing') && emit('focus', edge.target)"
        >
          <span>{{ otherNode(edge, 'outgoing')?.name ?? edge.target }}</span>
          <small>{{ edge.kind }} · {{ edge.count }}×</small>
        </button>
      </section>

      <section v-if="callSites.length" class="relation-section">
        <h3>Locations <span>{{ callSites.length }}</span></h3>
        <div v-for="({ edge, site }, index) in callSites" :key="`${edge.id}:${index}`" class="site">
          <span>{{ site.path }}</span>
          <small>{{ site.line ?? '?' }}:{{ site.column ?? '?' }} · {{ edge.confidence }}</small>
        </div>
      </section>
    </template>
  </aside>
</template>

<style scoped>
.details {
  background: var(--panel);
  border-left: 1px solid var(--line);
  height: 100%;
  overflow: auto;
  padding: 20px;
}

.empty {
  color: var(--muted);
  display: grid;
  gap: 6px;
  padding-top: 24px;
}

.node-header {
  border-bottom: 1px solid var(--line);
  padding-bottom: 16px;
}

.node-header h2 {
  font-size: 19px;
  margin: 9px 0 4px;
  overflow-wrap: anywhere;
}

.node-header p {
  color: var(--muted);
  font-family: var(--mono);
  font-size: 11px;
  margin: 0;
  overflow-wrap: anywhere;
}

.kind {
  background: var(--accent-soft);
  border-radius: 999px;
  color: var(--accent-strong);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: .06em;
  padding: 4px 7px;
  text-transform: uppercase;
}

.facts {
  display: grid;
  gap: 10px;
  margin: 16px 0;
}

.facts div {
  display: grid;
  gap: 3px;
}

.facts dt {
  color: var(--muted);
  font-size: 10px;
  text-transform: uppercase;
}

.facts dd {
  font-family: var(--mono);
  font-size: 11px;
  margin: 0;
  overflow-wrap: anywhere;
}

.relation-section {
  border-top: 1px solid var(--line);
  padding: 15px 0;
}

.relation-section h3 {
  font-size: 12px;
  margin: 0 0 8px;
  text-transform: uppercase;
}

.relation-section h3 span {
  color: var(--muted);
  font-weight: 500;
}

.relation {
  background: transparent;
  border: 0;
  border-radius: 6px;
  color: var(--text);
  cursor: pointer;
  display: grid;
  gap: 2px;
  padding: 7px;
  text-align: left;
  width: 100%;
}

.relation:hover,
.relation:focus-visible {
  background: var(--surface-raised);
}

.relation span {
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.relation small,
.site small,
.empty-line {
  color: var(--muted);
  font-size: 10px;
}

.empty-line {
  margin: 0 7px;
}

.site {
  display: grid;
  font-family: var(--mono);
  font-size: 10px;
  gap: 2px;
  padding: 5px 7px;
}

.site span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
