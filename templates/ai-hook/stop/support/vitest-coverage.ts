// jt-ai-hook
// Resolves project coverage rules in an isolated process, keeping hook stdout protocol-safe.

import { createRequire } from 'node:module'
import { isAbsolute, join, relative, resolve, sep } from 'node:path'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

const OUTPUT_MARKER = '__JT_VITEST_COVERAGE_FILES__'

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function stringArray(value: unknown): string[] | undefined {
  return Array.isArray(value) && value.every(item => typeof item === 'string')
    ? value
    : undefined
}

function slash(value: string): string {
  return value.split(sep).join('/')
}

function isWithinRoot(root: string, file: string): boolean {
  const pathFromRoot = relative(root, file)
  return pathFromRoot === '' || (
    pathFromRoot !== '..'
    && !pathFromRoot.startsWith(`..${sep}`)
    && !isAbsolute(pathFromRoot)
  )
}

async function selectCoverage(cwd: string, files: string[]): Promise<{
  files: string[]
  skipFull: boolean
}> {
  const requireFromProject = createRequire(join(cwd, 'package.json'))
  const vitestPackageJson = requireFromProject.resolve('vitest/package.json')
  const requireFromVitest = createRequire(vitestPackageJson)
  const [vitestNode, picomatchModule] = await Promise.all([
    import(pathToFileURL(requireFromProject.resolve('vitest/node')).href),
    import(pathToFileURL(requireFromVitest.resolve('picomatch')).href),
  ])
  if (typeof vitestNode.resolveConfig !== 'function')
    throw new TypeError('vitest/node does not export resolveConfig')

  const resolved: unknown = await vitestNode.resolveConfig({ root: cwd })
  if (!isRecord(resolved))
    throw new TypeError('Vitest returned an invalid resolved config')
  const config = isRecord(resolved.vitestConfig)
    ? resolved.vitestConfig
    : isRecord(resolved.test)
      ? resolved.test
      : resolved
  const coverage = isRecord(config.coverage) ? config.coverage : {}
  const root = typeof config.root === 'string' ? resolve(config.root) : resolve(cwd)
  const include = stringArray(coverage.include) ?? '**'
  const exclude = stringArray(coverage.exclude) ?? []
  const picomatch = picomatchModule.default
  if (!isRecord(picomatch) && typeof picomatch !== 'function')
    throw new TypeError('Vitest picomatch dependency is invalid')
  const isMatch = Reflect.get(picomatch, 'isMatch')
  if (typeof isMatch !== 'function')
    throw new TypeError('Vitest picomatch dependency does not export isMatch')

  return {
    files: files.filter((file) => {
      const absoluteFile = resolve(file)
      if (coverage.allowExternal !== true && !isWithinRoot(root, absoluteFile))
        return false
      return isMatch(slash(absoluteFile), include, {
        contains: true,
        dot: true,
        ignore: exclude,
      }) === true
    }),
    skipFull: coverage.skipFull === true,
  }
}

async function main(): Promise<void> {
  const [cwd, ...files] = process.argv.slice(2)
  try {
    if (!cwd)
      throw new TypeError('missing project root')
    const coverage = await selectCoverage(cwd, files)
    process.stdout.write(`${OUTPUT_MARKER}${JSON.stringify({ ...coverage, ok: true })}\n`)
  }
  catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    process.stdout.write(`${OUTPUT_MARKER}${JSON.stringify({
      error: message.slice(0, 1000),
      files,
      ok: false,
    })}\n`)
  }
}

void main()
