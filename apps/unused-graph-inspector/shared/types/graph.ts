export interface GraphSite {
  path: string
  line: number | null
  column: number | null
}

export interface GraphNode {
  id: string
  name: string
  qualifiedName: string
  kind: string
  language: string
  path: string
  line: number
  column: number
  degree: number
}

export interface GraphEdge {
  id: string
  source: string
  target: string
  kind: string
  confidence: 'exact' | 'potential'
  count: number
  sites: GraphSite[]
}

export interface GraphPayload {
  mode: 'overview' | 'call-flow'
  root: string
  focus: string | null
  depth: number
  truncated: boolean
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export interface GraphMeta {
  database: string
  generator: string
  root: string
  schemaVersion: number
  nodes: number
  edges: number
  nodeKinds: Record<string, number>
  edgeKinds: Record<string, number>
  diagnostics: number
}

export interface GraphSearchResult {
  id: string
  name: string
  qualifiedName: string
  kind: string
  path: string
  line: number
}

export interface CytoscapeElement {
  data: Record<string, string | number>
}
