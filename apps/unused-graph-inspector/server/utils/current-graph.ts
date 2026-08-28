import { loadGraphSnapshot } from './graph-store'

export function currentGraphSnapshot() {
  const config = useRuntimeConfig()
  return loadGraphSnapshot(config.graphDatabase)
}
