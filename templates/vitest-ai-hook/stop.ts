// jt-vitest-ai-hook
// Runs complete Vitest suites related to files changed during this AI turn.

import type { StateIdentity } from './runtime'
import { relative } from 'node:path'

import { safeFilePath } from './files'
import { isRecord, writeOutput } from './protocol'
import { runStage } from './runtime'
import { boundedOutput, executeVitest, resolveVitest } from './vitest'

runStage('Stop', (runtime) => {
  const collectedFiles = (identity: StateIdentity): string[] => {
    const files = new Set<string>()
    for (const name of runtime.stateFiles()) {
      if (!name.startsWith(runtime.recordPrefix(identity)))
        continue
      const record = runtime.readState(runtime.statePath(name))
      if (!isRecord(record) || !Array.isArray(record.files))
        continue
      for (const candidate of record.files) {
        const file = safeFilePath(runtime.cwd, candidate)
        if (file)
          files.add(file)
      }
    }
    return [...files].sort()
  }

  const finish = (
    identity: StateIdentity | null,
    output: Record<string, unknown>,
    clear: boolean,
  ): void => {
    if (clear && identity)
      runtime.clearTurnState(identity)
    writeOutput(output)
  }

  const identity = runtime.stateIdentity(false)
  const files = identity ? collectedFiles(identity) : []
  const relativeFiles = files.map(file => relative(runtime.cwd, file))
  if (files.length === 0) {
    runtime.writeLog('skipped-no-ai-edited-files', { files: [] })
    finish(identity, { continue: true }, true)
    return
  }

  const vitestBin = resolveVitest(runtime.cwd)
  if (!vitestBin) {
    runtime.writeLog('skipped-vitest-not-found', { files: relativeFiles })
    finish(identity, {
      continue: true,
      systemMessage: `[jt-vitest-ai-hook] Vitest not found. Install repository dependencies before relying on AI-hook tests. Log: ${runtime.logPath}`,
    }, true)
    return
  }

  const result = executeVitest(vitestBin, runtime.cwd, files)
  const detail = boundedOutput(result.stdout, result.stderr)
  if (result.status === 0) {
    runtime.writeLog('passed', { exitCode: result.status, files: relativeFiles })
    finish(identity, { continue: true }, true)
    return
  }

  const runtimeFailure = result.error || result.status === null
  const reason = [
    runtimeFailure
      ? '[jt-vitest-ai-hook] Vitest could not complete for AI-edited files.'
      : '[jt-vitest-ai-hook] Related Vitest suites failed for AI-edited files.',
    `Changed files: ${relativeFiles.join(', ')}`,
    `Log: ${runtime.logPath}`,
    detail || String(result.error?.message || 'Vitest exited without output.'),
  ].join('\n')
  const retryLimit = runtime.input.stop_hook_active === true
  runtime.writeLog(
    retryLimit ? 'failed-retry-limit' : runtimeFailure ? 'runtime-error' : 'failed-blocking',
    {
      exitCode: result.status,
      files: relativeFiles,
      signal: result.signal,
      output: detail.slice(0, 4000),
    },
  )
  finish(
    identity,
    retryLimit
      ? {
          continue: true,
          systemMessage: `${reason}\n\nStop hook already continued this turn; allowing stop to prevent an infinite loop.`,
        }
      : { decision: 'block', reason },
    retryLimit,
  )
})
