import type { CytoscapeElement, GraphPayload } from '../types/graph'

const NODE_COLORS: Record<string, string> = {
  class: '#8b5cf6',
  component: '#8b5cf6',
  constructor: '#6366f1',
  file: '#f59e0b',
  function: '#2563eb',
  method: '#2563eb',
  module: '#f59e0b',
  variable: '#059669',
}

const EDGE_COLORS: Record<string, string> = {
  calls: '#64748b',
  imports: '#d97706',
  instantiates: '#7c3aed',
  references: '#0f766e',
  reexport: '#db2777',
  'dynamic-import': '#dc2626',
}

export function graphElements(graph: GraphPayload): CytoscapeElement[] {
  const nodeIds = new Set(graph.nodes.map(node => node.id))
  const nodes = graph.nodes.map(node => ({
    data: {
      id: node.id,
      label: node.name,
      path: node.path,
      kind: node.kind,
      color: NODE_COLORS[node.kind] ?? '#64748b',
      width: Math.min(260, Math.max(96, node.name.length * 7.4 + 28)),
    },
  }))
  const edges = graph.edges
    .filter(edge => nodeIds.has(edge.source) && nodeIds.has(edge.target))
    .map((edge, index) => ({
      data: {
        id: `${edge.source}\u0000${edge.target}\u0000${edge.kind}\u0000${edge.confidence}\u0000${index}`,
        source: edge.source,
        target: edge.target,
        kind: edge.kind,
        confidence: edge.confidence,
        count: edge.count,
        color: EDGE_COLORS[edge.kind] ?? '#94a3b8',
      },
    }))
  return [...nodes, ...edges]
}
