import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import { afterEach, describe, expect, it } from 'vitest'

import { callFlowGraph, overviewGraph, searchGraph } from '../server/utils/graph-queries'
import { graphMeta, loadGraphSnapshot } from '../server/utils/graph-store'
import type { GraphSnapshot } from '../server/utils/graph-store'
import type { GraphEdge, GraphNode, GraphPayload } from '../shared/types/graph'
import { graphElements } from '../shared/utils/cytoscape-elements'

const temporaryDirectories: string[] = []

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { force: true, recursive: true })
  }
})

describe('graph database', () => {
  it('builds file overview and directed call flow from SQLite', () => {
    const database = fixtureDatabase()
    const snapshot = loadGraphSnapshot(database)
    expect(snapshot.edges.find(edge => edge.id === 'edge:3')).toMatchObject({
      confidence: 'potential',
      sites: [{ path: 'src/a.ts', line: 3, column: 3 }],
    })

    expect(graphMeta(snapshot)).toMatchObject({
      generator: 'jt call-graph',
      nodes: 4,
      edges: 3,
    })

    const overview = overviewGraph(snapshot)
    expect(overview.nodes.map(node => node.name)).toEqual(['src/a.ts', 'src/b.ts'])
    expect(overview.edges).toHaveLength(1)
    expect(overview.edges[0]).toMatchObject({ kind: 'imports', count: 1 })

    const callFlow = callFlowGraph(snapshot, 'symbol:caller', 2)
    expect(callFlow.nodes.map(node => node.name)).toEqual(['caller', 'callee'])
    expect(callFlow.edges).toHaveLength(1)
    expect(callFlow.edges[0]).toMatchObject({
      source: 'symbol:caller',
      target: 'symbol:callee',
      kind: 'calls',
      count: 2,
    })
    expect(searchGraph(snapshot, 'call').map(result => result.id)).toEqual([
      'symbol:caller',
      'symbol:callee',
    ])
  })

  it('drops dangling renderer edges and keeps IDs unique', () => {
    const graph: GraphPayload = {
      mode: 'call-flow',
      root: '/fixture',
      focus: 'one',
      depth: 1,
      truncated: false,
      nodes: [
        {
          id: 'one',
          name: 'one',
          qualifiedName: 'one',
          kind: 'function',
          language: 'typescript',
          path: 'src/a.ts',
          line: 1,
          column: 1,
          degree: 1,
        },
        {
          id: 'two',
          name: 'two',
          qualifiedName: 'two',
          kind: 'function',
          language: 'typescript',
          path: 'src/b.ts',
          line: 1,
          column: 1,
          degree: 1,
        },
      ],
      edges: [
        { id: 'edge:1', source: 'one', target: 'two', kind: 'calls', confidence: 'exact', count: 1, sites: [] },
        { id: 'edge:2', source: 'one', target: 'missing', kind: 'calls', confidence: 'exact', count: 1, sites: [] },
      ],
    }
    const elements = graphElements(graph)
    expect(elements).toHaveLength(3)
    expect(new Set(elements.map(element => element.data.id)).size).toBe(3)
  })

  it('limits the overview before it reaches ELK', () => {
    const nodes = Array.from({ length: 250 }, (_, index): GraphNode => ({
      id: `file:${index}`,
      name: `index-${index}.ts`,
      qualifiedName: `packages/package-${index}/index.ts`,
      kind: 'file',
      language: 'typescript',
      path: `packages/package-${index}/index.ts`,
      line: 1,
      column: 1,
      degree: 0,
    }))
    const edges = nodes.slice(1).map((node, index): GraphEdge => ({
      id: `edge:${index}`,
      source: nodes[index]!.id,
      target: node.id,
      kind: 'imports',
      confidence: 'exact',
      count: 1,
      sites: [],
    }))
    const snapshot: GraphSnapshot = {
      database: '/fixture/graph.db',
      metadata: new Map([
        ['generator', 'jt call-graph'],
        ['schema_version', '1'],
      ]),
      root: '/fixture',
      nodes,
      edges,
      nodeById: new Map(nodes.map(node => [node.id, node])),
    }

    const overview = overviewGraph(snapshot)
    expect(overview.truncated).toBe(true)
    expect(overview.nodes.length).toBeLessThanOrEqual(200)
    expect(overview.edges.length).toBeLessThanOrEqual(600)
  })

  it('rejects a database outside the jt graph contract', () => {
    const wrongVersion = fixtureDatabase()
    updateMetadata(wrongVersion, 'schema_version', '2')
    expect(() => loadGraphSnapshot(wrongVersion)).toThrow('Run jt call-graph again')

    const wrongGenerator = fixtureDatabase()
    updateMetadata(wrongGenerator, 'generator', 'another tool')
    expect(() => loadGraphSnapshot(wrongGenerator)).toThrow('generator must be jt call-graph')

    const missingColumn = invalidMetadataDatabase()
    expect(() => loadGraphSnapshot(missingColumn)).toThrow('project_metadata is missing columns: value')
  })
})

function fixtureDatabase(): string {
  const directory = mkdtempSync(join(tmpdir(), 'unused-graph-inspector-'))
  temporaryDirectories.push(directory)
  const path = join(directory, 'graph.db')
  const database = new DatabaseSync(path)
  database.exec(`
    CREATE TABLE project_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE nodes (
      id TEXT PRIMARY KEY,
      kind TEXT NOT NULL,
      name TEXT NOT NULL,
      qualified_name TEXT NOT NULL,
      file_path TEXT NOT NULL,
      language TEXT NOT NULL,
      start_line INTEGER NOT NULL,
      start_column INTEGER NOT NULL
    );
    CREATE TABLE edges (
      id INTEGER PRIMARY KEY,
      source TEXT NOT NULL,
      target TEXT NOT NULL,
      kind TEXT NOT NULL,
      metadata TEXT,
      line INTEGER,
      col INTEGER,
      provenance TEXT
    );
    INSERT INTO project_metadata VALUES ('generator', 'jt call-graph', 0);
    INSERT INTO project_metadata VALUES ('schema_version', '1', 0);
    INSERT INTO project_metadata VALUES ('root', '/fixture', 0);
    INSERT INTO nodes VALUES ('file:a', 'file', 'a.ts', 'src/a.ts', 'src/a.ts', 'typescript', 1, 0);
    INSERT INTO nodes VALUES ('file:b', 'file', 'b.ts', 'src/b.ts', 'src/b.ts', 'typescript', 1, 0);
    INSERT INTO nodes VALUES ('symbol:caller', 'function', 'caller', 'caller', 'src/a.ts', 'typescript', 3, 0);
    INSERT INTO nodes VALUES ('symbol:callee', 'function', 'callee', 'callee', 'src/b.ts', 'typescript', 5, 0);
    INSERT INTO edges VALUES (1, 'file:a', 'file:b', 'import', NULL, 1, 0, 'exact');
    INSERT INTO edges VALUES (
      2,
      'symbol:caller',
      'symbol:callee',
      'calls',
      '{"confidence":"exact","count":2,"sites":[{"path":"src/a.ts","line":4,"column":3}]}',
      4,
      2,
      'exact'
    );
    INSERT INTO edges VALUES (3, 'file:a', 'symbol:caller', 'contains', NULL, 3, 2, 'potential');
  `)
  database.close()
  return path
}

function updateMetadata(path: string, key: string, value: string) {
  const database = new DatabaseSync(path)
  database.prepare('UPDATE project_metadata SET value = ? WHERE key = ?').run(value, key)
  database.close()
}

function invalidMetadataDatabase(): string {
  const directory = mkdtempSync(join(tmpdir(), 'unused-graph-inspector-invalid-'))
  temporaryDirectories.push(directory)
  const path = join(directory, 'graph.db')
  const database = new DatabaseSync(path)
  database.exec(`
    CREATE TABLE project_metadata (key TEXT PRIMARY KEY);
    CREATE TABLE nodes (id TEXT PRIMARY KEY);
    CREATE TABLE edges (id INTEGER PRIMARY KEY);
  `)
  database.close()
  return path
}
