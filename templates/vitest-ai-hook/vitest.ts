// jt-vitest-ai-hook

import type { SpawnSyncReturns } from 'node:child_process'
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join, relative, sep } from 'node:path'
import process from 'node:process'

const COVERAGE_OUTPUT_MARKER = '__JT_VITEST_COVERAGE_FILES__'

export interface CoverageSelection {
  excludedFiles: string[]
  files: string[]
  warning: string | null
}

export function resolveVitest(cwd: string): string | null {
  try {
    const requireFromProject = createRequire(join(cwd, 'package.json'))
    const packageJson = requireFromProject.resolve('vitest/package.json')
    return join(dirname(packageJson), 'vitest.mjs')
  }
  catch {
    const fallback = join(cwd, 'node_modules/vitest/vitest.mjs')
    return existsSync(fallback) ? fallback : null
  }
}

export function executeVitest(
  vitestBin: string,
  cwd: string,
  files: string[],
  coverageFiles: string[],
): SpawnSyncReturns<string> {
  const coverageArgs = coverageFiles.length > 0
    ? [
        '--coverage.enabled',
        ...coverageFiles.map(file =>
          `--coverage.include=${relative(cwd, file).split(sep).join('/')}`,
        ),
        '--coverage.reporter=text',
      ]
    : ['--coverage.enabled=false']

  return spawnSync(
    process.execPath,
    [
      vitestBin,
      'related',
      ...files,
      '--run',
      '--reporter=agent',
      ...coverageArgs,
      '--silent',
      '--no-color',
      '--passWithNoTests',
    ],
    {
      cwd,
      encoding: 'utf8',
      env: { ...process.env, NO_COLOR: '1' },
      maxBuffer: 10 * 1024 * 1024,
      timeout: 120_000,
    },
  )
}

export function selectCoverageFiles(cwd: string, files: string[]): CoverageSelection {
  const helper = join(cwd, '.codex/hooks/jt-vitest/coverage.ts')
  const result = spawnSync(
    'pnpm',
    ['--dir', cwd, 'exec', 'tsx', helper, cwd, ...files],
    {
      cwd,
      encoding: 'utf8',
      env: { ...process.env, NO_COLOR: '1' },
      maxBuffer: 2 * 1024 * 1024,
      timeout: 30_000,
    },
  )
  const output = String(result.stdout || '')
  const markerIndex = output.lastIndexOf(COVERAGE_OUTPUT_MARKER)
  if (result.status === 0 && markerIndex >= 0) {
    try {
      const line = output
        .slice(markerIndex + COVERAGE_OUTPUT_MARKER.length)
        .split('\n', 1)[0]
      const parsed: unknown = JSON.parse(line)
      if (
        parsed !== null
        && typeof parsed === 'object'
        && !Array.isArray(parsed)
        && Reflect.get(parsed, 'ok') === true
        && Array.isArray(Reflect.get(parsed, 'files'))
      ) {
        const allowed = new Set(files)
        const selected = Reflect.get(parsed, 'files')
          .filter((file: unknown): file is string => typeof file === 'string' && allowed.has(file))
        const selectedSet = new Set(selected)
        return {
          excludedFiles: files.filter(file => !selectedSet.has(file)),
          files: selected,
          warning: null,
        }
      }
      const error = Reflect.get(parsed, 'error')
      if (typeof error === 'string')
        throw new Error(error)
    }
    catch (error) {
      const detail = error instanceof Error ? error.message : String(error)
      return {
        excludedFiles: [],
        files,
        warning: `Could not resolve project coverage filters; using all AI-edited files. ${detail}`,
      }
    }
  }

  const detail = boundedOutput(result.stdout, result.stderr)
  return {
    excludedFiles: [],
    files,
    warning: `Could not resolve project coverage filters; using all AI-edited files. ${detail || result.error?.message || 'filter process failed'}`,
  }
}

export function boundedOutput(stdout: unknown, stderr: unknown): string {
  const output = [stdout, stderr]
    .map(value => String(value || '').trim())
    .filter(Boolean)
    .join('\n')
  return output.length <= 12_000
    ? output
    : `${output.slice(0, 12_000)}\n... Vitest output truncated`
}
