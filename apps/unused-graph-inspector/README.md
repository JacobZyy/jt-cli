# `@jacob-z/unused-graph-inspector`

Nuxt SSR inspector for the SQLite graph produced by `jt call-graph`.

Generate graph data first:

```bash
jt call-graph /path/to/project
```

Run the inspector from this workspace:

```bash
NUXT_GRAPH_DATABASE=/path/to/project/.nlab/unused-graph.db \
  pnpm --filter @jacob-z/unused-graph-inspector dev
```

Open <http://127.0.0.1:3000>. The initial Overview aggregates imports by project area. Search for a symbol to render its directed caller/callee flow with ELK.

The server opens SQLite read-only. Browser requests are limited to fixed metadata, overview, search, and subgraph endpoints; raw SQL is never accepted.

The npm executable and `jt graph` launcher are intentionally deferred.
