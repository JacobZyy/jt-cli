import { realpathSync, statSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import type { GraphEdge, GraphMeta, GraphNode, GraphSite } from '../../shared/types/graph'

const DATABASE_GENERATOR = 'jt call-graph'
const DATABASE_SCHEMA_VERSION = 1

interface NodeRow {
  id: string
  kind: string
  name: string
  qualified_name: string
  file_path: string
  language: string
  start_line: number
  start_column: number
}

interface EdgeRow {
  id: number
  source: string
  target: string
  kind: string
  metadata: string | null
  line: number | null
  col: number | null
  provenance: string | null
}

export interface GraphSnapshot {
  database: string
  metadata: Map<string, string>
  root: string
  nodes: GraphNode[]
  edges: GraphEdge[]
  nodeById: Map<string, GraphNode>
}

let cache: { key: string, snapshot: GraphSnapshot } | undefined

export function resolveDatabasePath(configuredPath: string): string {
  if (!configuredPath.trim()) {
    throw new Error('NUXT_GRAPH_DATABASE must point to .nlab/unused-graph.db')
  }
  const database = realpathSync(resolve(configuredPath))
  if (!statSync(database).isFile()) {
    throw new Error(`Graph database is not a regular file: ${database}`)
  }
  return database
}

export function loadGraphSnapshot(configuredPath: string): GraphSnapshot {
  const databasePath = resolveDatabasePath(configuredPath)
  const stats = statSync(databasePath)
  const cacheKey = `${databasePath}\u0000${stats.size}\u0000${stats.mtimeMs}`
  if (cache?.key === cacheKey) {
    return cache.snapshot
  }

  const database = new DatabaseSync(databasePath, {
    allowExtension: false,
    defensive: true,
    readOnly: true,
  })
  try {
    const tables = new Set(
      (database.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all() as Array<{ name: string }>)
        .map(row => row.name),
    )
    for (const table of ['project_metadata', 'nodes', 'edges']) {
      if (!tables.has(table)) {
        throw unsupportedDatabase(`missing ${table} table`)
      }
    }
    requireColumns(database, 'project_metadata', ['key', 'value'])
    requireColumns(database, 'nodes', [
      'id', 'kind', 'name', 'qualified_name', 'file_path', 'language', 'start_line', 'start_column',
    ])
    requireColumns(database, 'edges', [
      'id', 'source', 'target', 'kind', 'metadata', 'line', 'col', 'provenance',
    ])

    const metadata = new Map<string, string>()
    for (const row of database.prepare('SELECT key, value FROM project_metadata').all() as Array<{ key: string, value: string }>) {
      metadata.set(row.key, row.value)
    }
    if (metadata.get('generator') !== DATABASE_GENERATOR) {
      throw unsupportedDatabase(`generator must be ${DATABASE_GENERATOR}`)
    }
    const schemaVersion = Number(metadata.get('schema_version'))
    if (!Number.isInteger(schemaVersion) || schemaVersion !== DATABASE_SCHEMA_VERSION) {
      throw unsupportedDatabase(`schema_version must be ${DATABASE_SCHEMA_VERSION}`)
    }

    const nodes = (database.prepare(
      `SELECT id, kind, name, qualified_name, file_path, language,
              start_line, start_column
       FROM nodes
       ORDER BY id`,
    ).all() as unknown as NodeRow[]).map(row => ({
      id: row.id,
      name: row.name,
      qualifiedName: row.qualified_name,
      kind: row.kind,
      language: row.language,
      path: row.file_path,
      line: Number(row.start_line),
      column: Number(row.start_column) + 1,
      degree: 0,
    }))
    const nodeById = new Map(nodes.map(node => [node.id, node]))
    const edges = (database.prepare(
      `SELECT id, source, target, kind, metadata, line, col, provenance
       FROM edges
       ORDER BY source, target, kind, id`,
    ).all() as unknown as EdgeRow[])
      .filter(row => nodeById.has(row.source) && nodeById.has(row.target))
      .map(row => graphEdge(row, nodeById.get(row.source)!))

    for (const edge of edges) {
      nodeById.get(edge.source)!.degree += 1
      nodeById.get(edge.target)!.degree += 1
    }

    const snapshot = {
      database: databasePath,
      metadata,
      root: metadata.get('root') ?? dirname(dirname(databasePath)),
      nodes,
      edges,
      nodeById,
    }
    cache = { key: cacheKey, snapshot }
    return snapshot
  }
  finally {
    database.close()
  }
}

export function graphMeta(snapshot: GraphSnapshot): GraphMeta {
  return {
    database: snapshot.database,
    generator: snapshot.metadata.get('generator') ?? 'CodeGraph',
    root: snapshot.root,
    schemaVersion: Number(snapshot.metadata.get('schema_version')),
    nodes: snapshot.nodes.length,
    edges: snapshot.edges.length,
    nodeKinds: counts(snapshot.nodes.map(node => node.kind)),
    edgeKinds: counts(snapshot.edges.map(edge => edge.kind)),
    diagnostics: jsonArrayLength(snapshot.metadata.get('diagnostics')),
  }
}

function graphEdge(row: EdgeRow, source: GraphNode): GraphEdge {
  const metadata = edgeMetadata(row.metadata)
  const sites = metadata.sites.length > 0
    ? metadata.sites
    : row.line === null
      ? []
      : [{ path: source.path, line: Number(row.line), column: row.col === null ? null : Number(row.col) + 1 }]
  return {
    id: `edge:${row.id}`,
    source: row.source,
    target: row.target,
    kind: normalizeEdgeKind(row.kind),
    confidence: metadata.confidence ?? (row.provenance === 'heuristic' || row.provenance === 'potential' ? 'potential' : 'exact'),
    count: metadata.count ?? Math.max(1, sites.length),
    sites,
  }
}

function edgeMetadata(value: string | null): {
  confidence?: 'exact' | 'potential'
  count?: number
  sites: GraphSite[]
} {
  if (!value) {
    return { sites: [] }
  }
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>
    const confidence = parsed.confidence === 'potential' ? 'potential' : parsed.confidence === 'exact' ? 'exact' : undefined
    const count = typeof parsed.count === 'number' && Number.isFinite(parsed.count) && parsed.count > 0
      ? Math.floor(parsed.count)
      : undefined
    const sites = Array.isArray(parsed.sites)
      ? parsed.sites.flatMap((site) => {
          if (!site || typeof site !== 'object') {
            return []
          }
          const record = site as Record<string, unknown>
          if (typeof record.path !== 'string') {
            return []
          }
          return [{
            path: record.path,
            line: typeof record.line === 'number' ? record.line : null,
            column: typeof record.column === 'number' ? record.column : null,
          }]
        })
      : []
    return { confidence, count, sites }
  }
  catch {
    return { sites: [] }
  }
}

function requireColumns(database: DatabaseSync, table: string, required: string[]) {
  const columns = new Set(
    (database.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name: string }>).map(row => row.name),
  )
  const missing = required.filter(column => !columns.has(column))
  if (missing.length > 0) {
    throw unsupportedDatabase(`${table} is missing columns: ${missing.join(', ')}`)
  }
}

function unsupportedDatabase(reason: string): Error {
  return new Error(`Unsupported graph database (${reason}). Run jt call-graph again.`)
}

function normalizeEdgeKind(kind: string): string {
  if (kind === 'call') return 'calls'
  if (kind === 'import') return 'imports'
  return kind
}

function counts(values: string[]): Record<string, number> {
  const result = Object.create(null) as Record<string, number>
  for (const value of values) {
    result[value] = (result[value] ?? 0) + 1
  }
  return Object.fromEntries(Object.entries(result).sort(([left], [right]) => left.localeCompare(right)))
}

function jsonArrayLength(value: string | undefined): number {
  if (!value) return 0
  try {
    const parsed = JSON.parse(value)
    return Array.isArray(parsed) ? parsed.length : 0
  }
  catch {
    return 0
  }
}
