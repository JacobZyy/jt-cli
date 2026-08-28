import type { GraphNode, GraphPayload, GraphSearchResult } from '#shared/types/graph'

export function useInspectorGraph(initialGraph: GraphPayload) {
  const overview = initialGraph
  const graph = shallowRef(initialGraph)
  const query = shallowRef('')
  const searchResults = shallowRef<GraphSearchResult[]>([])
  const selectedId = shallowRef<string | null>(null)
  const focusId = shallowRef<string | null>(null)
  const depth = shallowRef(1)
  const loading = shallowRef(false)
  const error = shallowRef<string | null>(null)
  let focusRequest = 0

  const selectedNode = computed<GraphNode | null>(() => (
    graph.value.nodes.find(node => node.id === selectedId.value) ?? null
  ))

  watch(query, (value, _previous, onCleanup) => {
    const trimmed = value.trim()
    if (!trimmed) {
      searchResults.value = []
      return
    }
    const controller = new AbortController()
    const timer = setTimeout(async () => {
      try {
        searchResults.value = await $fetch<GraphSearchResult[]>('/api/search', {
          query: { q: trimmed },
          signal: controller.signal,
        })
      }
      catch (caught) {
        if (!controller.signal.aborted) {
          error.value = errorMessage(caught)
        }
      }
    }, 140)
    onCleanup(() => {
      clearTimeout(timer)
      controller.abort()
    })
  })

  async function focus(nodeId: string) {
    const request = ++focusRequest
    loading.value = true
    error.value = null
    try {
      const result = await $fetch<GraphPayload>('/api/subgraph', {
        query: { id: nodeId, depth: depth.value },
      })
      if (request !== focusRequest) return
      graph.value = result
      focusId.value = nodeId
      selectedId.value = nodeId
      searchResults.value = []
    }
    catch (caught) {
      if (request === focusRequest) {
        error.value = errorMessage(caught)
      }
    }
    finally {
      if (request === focusRequest) {
        loading.value = false
      }
    }
  }

  function showOverview() {
    focusRequest += 1
    graph.value = overview
    focusId.value = null
    selectedId.value = null
    loading.value = false
    error.value = null
  }

  function selectNode(nodeId: string | null) {
    selectedId.value = nodeId
  }

  function updateDepth(value: number) {
    depth.value = Math.max(1, Math.min(4, value))
    if (focusId.value) {
      void focus(focusId.value)
    }
  }

  return {
    depth,
    error: shallowReadonly(error),
    graph: shallowReadonly(graph),
    loading: shallowReadonly(loading),
    query,
    searchResults: shallowReadonly(searchResults),
    selectedId: shallowReadonly(selectedId),
    selectedNode,
    focus,
    selectNode,
    showOverview,
    updateDepth,
  }
}

function errorMessage(error: unknown): string {
  if (error && typeof error === 'object' && 'data' in error) {
    const data = (error as { data?: { statusMessage?: unknown } }).data
    if (typeof data?.statusMessage === 'string') return data.statusMessage
  }
  return error instanceof Error ? error.message : 'Graph request failed'
}
