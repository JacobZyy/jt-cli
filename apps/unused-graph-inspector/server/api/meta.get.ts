import { graphMeta } from '../utils/graph-store'

export default defineEventHandler(() => {
  try {
    return graphMeta(currentGraphSnapshot())
  }
  catch (error) {
    throw createError({
      statusCode: 500,
      statusMessage: error instanceof Error ? error.message : 'Cannot read graph database',
    })
  }
})
