// jt-vitest-ai-hook

import type { SpawnSyncReturns } from 'node:child_process'
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import process from 'node:process'

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
): SpawnSyncReturns<string> {
  return spawnSync(
    process.execPath,
    [
      vitestBin,
      'related',
      ...files,
      '--run',
      '--reporter=agent',
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

export function boundedOutput(stdout: unknown, stderr: unknown): string {
  const output = [stdout, stderr]
    .map(value => String(value || '').trim())
    .filter(Boolean)
    .join('\n')
  return output.length <= 12_000
    ? output
    : `${output.slice(0, 12_000)}\n... Vitest output truncated`
}
