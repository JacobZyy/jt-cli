import { searchGraph } from '../utils/graph-queries'

export default defineEventHandler((event) => {
  const query = getQuery(event).q
  if (typeof query !== 'string') return []
  try {
    return searchGraph(currentGraphSnapshot(), query)
  }
  catch (error) {
    throw createError({
      statusCode: 500,
      statusMessage: error instanceof Error ? error.message : 'Cannot search graph',
    })
  }
})
