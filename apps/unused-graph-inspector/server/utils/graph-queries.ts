import type { GraphEdge, GraphNode, GraphPayload, GraphSearchResult, GraphSite } from '../../shared/types/graph'
import type { GraphSnapshot } from './graph-store'

const OVERVIEW_EDGE_KINDS = new Set(['imports', 'reexport', 'dynamic-import'])
const CALL_EDGE_KINDS = new Set(['calls', 'instantiates', 'renders', 'event-handler'])
const MAX_OVERVIEW_NODES = 200
const MAX_OVERVIEW_EDGES = 600
const MAX_CALL_FLOW_NODES = 240
const MAX_CALL_FLOW_EDGES = 800

export function overviewGraph(snapshot: GraphSnapshot): GraphPayload {
  const areas = new Map<string, GraphNode>()
  for (const node of snapshot.nodes) {
    const area = graphArea(node.path)
    if (!areas.has(area)) {
      areas.set(area, {
        id: `area::${area}`,
        name: area,
        qualifiedName: area,
        kind: 'module',
        language: 'mixed',
        path: area,
        line: 1,
        column: 1,
        degree: 0,
      })
    }
  }
  const areaById = new Map([...areas.values()].map(node => [node.id, node]))

  const edges = new Map<string, GraphEdge>()
  for (const edge of snapshot.edges) {
    if (!OVERVIEW_EDGE_KINDS.has(edge.kind)) continue
    const sourcePath = snapshot.nodeById.get(edge.source)?.path
    const targetPath = snapshot.nodeById.get(edge.target)?.path
    if (!sourcePath || !targetPath) continue
    const source = areas.get(graphArea(sourcePath))!
    const target = areas.get(graphArea(targetPath))!
    if (source.id === target.id) continue
    const key = `${source.id}\u0000${target.id}\u0000${edge.kind}\u0000${edge.confidence}`
    const existing = edges.get(key)
    if (existing) {
      existing.count += edge.count
      existing.sites = mergeSites(existing.sites, edge.sites)
    }
    else {
      edges.set(key, {
        ...edge,
        id: `overview:${source.id}:${target.id}:${edge.kind}:${edge.confidence}`,
        source: source.id,
        target: target.id,
        sites: [...edge.sites],
      })
    }
  }

  for (const edge of edges.values()) {
    const source = areaById.get(edge.source)
    const target = areaById.get(edge.target)
    if (source) source.degree += 1
    if (target) target.degree += 1
  }
  const connected = new Set([...edges.values()].flatMap(edge => [edge.source, edge.target]))
  const rankedNodes = [...areas.values()]
    .filter(node => connected.has(node.id))
    .sort((left, right) => right.degree - left.degree || compareNodes(left, right))
  const nodes = rankedNodes.slice(0, MAX_OVERVIEW_NODES)
  const selected = new Set(nodes.map(node => node.id))
  const candidateEdges = [...edges.values()]
    .filter(edge => selected.has(edge.source) && selected.has(edge.target))
    .sort((left, right) => right.count - left.count || compareEdges(left, right))
  const visibleEdges = candidateEdges.slice(0, MAX_OVERVIEW_EDGES).sort(compareEdges)

  return {
    mode: 'overview',
    root: snapshot.root,
    focus: null,
    depth: 1,
    truncated: rankedNodes.length > nodes.length || candidateEdges.length > visibleEdges.length,
    nodes: nodes.sort(compareNodes),
    edges: visibleEdges,
  }
}

function graphArea(path: string): string {
  const segments = path.split('/').filter(Boolean)
  return segments.length <= 2 ? path : segments.slice(0, 2).join('/')
}

export function callFlowGraph(snapshot: GraphSnapshot, focus: string, requestedDepth: number): GraphPayload {
  if (!snapshot.nodeById.has(focus)) {
    throw new Error(`Graph node not found: ${focus}`)
  }
  const depth = Math.max(1, Math.min(4, requestedDepth))
  const adjacent = new Map<string, Set<string>>()
  for (const edge of snapshot.edges) {
    if (!CALL_EDGE_KINDS.has(edge.kind)) continue
    addAdjacent(adjacent, edge.source, edge.target)
    addAdjacent(adjacent, edge.target, edge.source)
  }

  const selected = new Set([focus])
  let frontier = [focus]
  let reachedNodeLimit = false
  for (let distance = 0; distance < depth; distance += 1) {
    const next = new Set<string>()
    for (const id of frontier) {
      for (const neighbor of adjacent.get(id) ?? []) {
        if (selected.size >= MAX_CALL_FLOW_NODES) {
          reachedNodeLimit = true
          break
        }
        if (!selected.has(neighbor)) {
          selected.add(neighbor)
          next.add(neighbor)
        }
      }
    }
    frontier = [...next].sort()
    if (frontier.length === 0 || reachedNodeLimit) break
  }
  const candidateEdges = snapshot.edges
    .filter(edge => CALL_EDGE_KINDS.has(edge.kind) && selected.has(edge.source) && selected.has(edge.target))
    .sort(compareEdges)
  const edges = candidateEdges.slice(0, MAX_CALL_FLOW_EDGES)

  return {
    mode: 'call-flow',
    root: snapshot.root,
    focus,
    depth,
    truncated: reachedNodeLimit || candidateEdges.length > edges.length,
    nodes: snapshot.nodes.filter(node => selected.has(node.id)).sort(compareNodes),
    edges,
  }
}

export function searchGraph(snapshot: GraphSnapshot, rawQuery: string): GraphSearchResult[] {
  const query = rawQuery.trim().toLocaleLowerCase()
  if (!query) return []
  return snapshot.nodes
    .filter(node => node.kind !== 'file')
    .filter(node => `${node.name}\n${node.qualifiedName}\n${node.path}`.toLocaleLowerCase().includes(query))
    .sort((left, right) => {
      const leftPrefix = left.name.toLocaleLowerCase().startsWith(query) ? 0 : 1
      const rightPrefix = right.name.toLocaleLowerCase().startsWith(query) ? 0 : 1
      return leftPrefix - rightPrefix || right.degree - left.degree || compareNodes(left, right)
    })
    .slice(0, 30)
    .map(node => ({
      id: node.id,
      name: node.name,
      qualifiedName: node.qualifiedName,
      kind: node.kind,
      path: node.path,
      line: node.line,
    }))
}

function addAdjacent(adjacency: Map<string, Set<string>>, source: string, target: string) {
  const neighbors = adjacency.get(source) ?? new Set<string>()
  neighbors.add(target)
  adjacency.set(source, neighbors)
}

function mergeSites(left: GraphSite[], right: GraphSite[]): GraphSite[] {
  return [...new Map([...left, ...right].map(site => [`${site.path}:${site.line}:${site.column}`, site])).values()]
    .sort((a, b) => `${a.path}:${a.line}:${a.column}`.localeCompare(`${b.path}:${b.line}:${b.column}`))
}

function compareNodes(left: GraphNode, right: GraphNode): number {
  return `${left.path}\u0000${left.line}\u0000${left.name}\u0000${left.id}`
    .localeCompare(`${right.path}\u0000${right.line}\u0000${right.name}\u0000${right.id}`)
}

function compareEdges(left: GraphEdge, right: GraphEdge): number {
  return `${left.source}\u0000${left.target}\u0000${left.kind}\u0000${left.id}`
    .localeCompare(`${right.source}\u0000${right.target}\u0000${right.kind}\u0000${right.id}`)
}
