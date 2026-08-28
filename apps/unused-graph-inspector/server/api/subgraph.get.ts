import { callFlowGraph } from '../utils/graph-queries'

export default defineEventHandler((event) => {
  const query = getQuery(event)
  if (typeof query.id !== 'string' || !query.id) {
    throw createError({ statusCode: 400, statusMessage: 'id is required' })
  }
  const depth = typeof query.depth === 'string' ? Number.parseInt(query.depth, 10) : 2
  try {
    return callFlowGraph(currentGraphSnapshot(), query.id, Number.isFinite(depth) ? depth : 2)
  }
  catch (error) {
    throw createError({
      statusCode: error instanceof Error && error.message.startsWith('Graph node not found:') ? 404 : 500,
      statusMessage: error instanceof Error ? error.message : 'Cannot build call flow',
    })
  }
})
