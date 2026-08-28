import { overviewGraph } from '../utils/graph-queries'

export default defineEventHandler(() => {
  try {
    return overviewGraph(currentGraphSnapshot())
  }
  catch (error) {
    throw createError({
      statusCode: 500,
      statusMessage: error instanceof Error ? error.message : 'Cannot build graph overview',
    })
  }
})
