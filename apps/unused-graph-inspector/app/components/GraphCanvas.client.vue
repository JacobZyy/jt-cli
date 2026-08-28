<script setup lang="ts">
import cytoscape, { type Core, type LayoutOptions, type StylesheetJson } from 'cytoscape'
import elk from 'cytoscape-elk'
import { nextTick, onBeforeUnmount, onMounted, useTemplateRef, watch } from 'vue'

import type { GraphPayload } from '#shared/types/graph'
import { graphElements } from '#shared/utils/cytoscape-elements'

const props = defineProps<{
  graph: GraphPayload
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [id: string | null]
}>()

const container = useTemplateRef<HTMLDivElement>('container')
const state = shallowRef<'waiting' | 'rendering' | 'ready' | 'error'>('waiting')
let graph: Core | undefined
let resizeObserver: ResizeObserver | undefined

cytoscape.use(elk)

onMounted(async () => {
  await nextTick()
  renderGraph()
  resizeObserver = new ResizeObserver(() => graph?.resize())
  if (container.value) resizeObserver.observe(container.value)
})

watch(() => props.graph, renderGraph)
watch(() => props.selectedId, applySelection)

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  graph?.destroy()
})

function renderGraph() {
  if (!container.value) return
  state.value = 'rendering'
  try {
    graph?.destroy()
    graph = cytoscape({
      container: container.value,
      elements: graphElements(props.graph),
      minZoom: 0.08,
      maxZoom: 4,
      style: GRAPH_STYLES,
    })
    graph.on('tap', 'node', event => emit('select', event.target.id()))
    graph.on('tap', event => {
      if (event.target === graph) emit('select', null)
    })
    graph.on('mouseover', 'node', event => highlightNeighborhood(event.target.id()))
    graph.on('mouseout', 'node', applySelection)
    const layoutOptions = {
      name: 'elk',
      animate: false,
      fit: true,
      padding: 38,
      elk: {
        algorithm: 'layered',
        'elk.direction': 'RIGHT',
        'elk.edgeRouting': 'ORTHOGONAL',
        'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
        'elk.layered.nodePlacement.strategy': 'NETWORK_SIMPLEX',
        'elk.layered.spacing.nodeNodeBetweenLayers': '80',
        'elk.spacing.nodeNode': '34',
      },
    } as unknown as LayoutOptions
    const layout = graph.layout(layoutOptions)
    layout.on('layoutstop', () => {
      graph?.fit(undefined, 38)
      state.value = 'ready'
    })
    layout.run()
    applySelection()
  }
  catch (error) {
    state.value = 'error'
    console.error('Cannot render graph', error)
  }
}

function applySelection() {
  if (!graph) return
  graph.elements().removeClass('muted selected neighbor')
  if (!props.selectedId) return
  const selected = graph.getElementById(props.selectedId)
  if (selected.empty()) return
  const neighborhood = selected.closedNeighborhood()
  graph.elements().not(neighborhood).addClass('muted')
  neighborhood.addClass('neighbor')
  selected.addClass('selected')
}

function highlightNeighborhood(nodeId: string) {
  if (!graph) return
  const node = graph.getElementById(nodeId)
  const neighborhood = node.closedNeighborhood()
  graph.elements().removeClass('muted neighbor')
  graph.elements().not(neighborhood).addClass('muted')
  neighborhood.addClass('neighbor')
}

const GRAPH_STYLES: StylesheetJson = [
  {
    selector: 'node',
    style: {
      'background-color': 'data(color)',
      'border-color': '#ffffff',
      'border-width': 1.5,
      color: '#ffffff',
      'font-family': 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      'font-size': 11,
      height: 36,
      label: 'data(label)',
      'min-zoomed-font-size': 7,
      shape: 'round-rectangle',
      'text-max-width': '230px',
      'text-overflow-wrap': 'anywhere',
      'text-wrap': 'ellipsis',
      'text-valign': 'center',
      'text-halign': 'center',
      width: 'data(width)',
    },
  },
  {
    selector: 'edge',
    style: {
      'curve-style': 'segments',
      'line-color': 'data(color)',
      opacity: 0.48,
      'target-arrow-color': 'data(color)',
      'target-arrow-shape': 'triangle',
      'arrow-scale': 0.72,
      width: 'mapData(count, 1, 20, 1, 4)',
    },
  },
  {
    selector: 'edge[confidence = "potential"]',
    style: {
      'line-style': 'dashed',
      opacity: 0.32,
    },
  },
  {
    selector: '.muted',
    style: {
      opacity: 0.08,
      'text-opacity': 0.08,
    },
  },
  {
    selector: 'node.selected',
    style: {
      'border-color': '#101828',
      'border-width': 4,
      'z-index': 20,
    },
  },
  {
    selector: '.neighbor',
    style: {
      opacity: 1,
    },
  },
]
</script>

<template>
  <div
    ref="container"
    class="graph-canvas"
    role="application"
    aria-label="Interactive directed code graph"
    :data-state="state"
  />
</template>

<style scoped>
.graph-canvas {
  background-color: #fbfcfe;
  background-image: radial-gradient(#d8dfeb 0.75px, transparent 0.75px);
  background-size: 18px 18px;
  height: 100%;
  min-height: 460px;
  width: 100%;
}
</style>
