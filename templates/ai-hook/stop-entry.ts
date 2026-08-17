// jt-ai-hook
// Discovers direct TypeScript runners and executes them concurrently.

import type { StopRunnerContext, StopRunnerModule, StopRunnerResult } from './stop/types'
import type { StateIdentity } from './runtime'
import { lstatSync, readdirSync } from 'node:fs'
import { basename, join, relative } from 'node:path'
import { pathToFileURL } from 'node:url'

import { safeFilePath } from './files'
import { isRecord, writeOutput } from './protocol'
import { runStage } from './runtime'

interface RunnerOutcome extends StopRunnerResult {
  name: string
}

function runnerFiles(cwd: string): string[] {
  const directory = safeFilePath(cwd, '.codex/hooks/jt-ai-hook/stop/runner')
  if (!directory)
    return []
  try {
    if (!lstatSync(directory).isDirectory())
      return []
    return readdirSync(directory, { withFileTypes: true })
      .filter(entry => entry.isFile() && entry.name.endsWith('.ts'))
      .map(entry => join(directory, entry.name))
      .sort((left, right) => left.localeCompare(right))
  }
  catch {
    return []
  }
}

function runnerName(file: string): string {
  return basename(file, '.ts')
}

function runnerTitle(name: string): string {
  if (name === 'eslint')
    return 'ESLint'
  if (name === 'vitest')
    return 'Vitest'
  return name
}

async function executeRunner(file: string, context: StopRunnerContext): Promise<RunnerOutcome> {
  const imported: unknown = await import(pathToFileURL(file).href)
  if (!isRecord(imported) || typeof imported.run !== 'function')
    throw new TypeError(`${runnerName(file)} does not export run(context)`)
  const result = await (imported as unknown as StopRunnerModule).run(context)
  if (!isRecord(result) || !['failed', 'passed', 'warning'].includes(String(result.status)))
    throw new TypeError(`${runnerName(file)} returned an invalid result`)
  return { ...result, name: runnerName(file) }
}

function section(outcome: RunnerOutcome): string {
  return `${runnerTitle(outcome.name)}\n${outcome.message || 'Runner failed without details.'}`
}

runStage('Stop', async (runtime) => {
  const collectFiles = (identity: StateIdentity): string[] => {
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

  const identity = runtime.stateIdentity(false)
  const files = identity ? collectFiles(identity) : []
  if (files.length === 0) {
    runtime.writeLog('skipped-no-ai-edited-files', { files: [] })
    if (identity)
      runtime.clearTurnState(identity)
    writeOutput({ continue: true, suppressOutput: true })
    return
  }

  const context: StopRunnerContext = {
    cwd: runtime.cwd,
    files,
    logPath: runtime.logPath,
    relativeFiles: files.map(file => relative(runtime.cwd, file)),
  }
  const discovered = runnerFiles(runtime.cwd)
  if (discovered.length === 0) {
    runtime.writeLog('skipped-no-runners', { files: context.relativeFiles })
    if (identity)
      runtime.clearTurnState(identity)
    writeOutput({
      continue: true,
      systemMessage: `[jt-ai-hook] No Stop runners found; validation was skipped. Log: ${runtime.logPath}`,
    })
    return
  }

  const settled = await Promise.allSettled(
    discovered.map(file => executeRunner(file, context)),
  )
  const outcomes = settled.map((result, index): RunnerOutcome => {
    if (result.status === 'fulfilled')
      return result.value
    const error = result.reason instanceof Error ? result.reason.message : String(result.reason)
    return {
      name: runnerName(discovered[index]),
      status: 'failed',
      message: `Runner could not complete. ${error}`,
    }
  })
  for (const outcome of outcomes) {
    runtime.writeLog(`runner-${outcome.status}`, {
      details: outcome.details || {},
      files: context.relativeFiles,
      message: outcome.message || null,
      runner: outcome.name,
    })
  }

  const failures = outcomes.filter(outcome => outcome.status === 'failed')
  const warnings = outcomes.filter(outcome => outcome.status === 'warning')
  if (failures.length > 0) {
    const reported = [...failures, ...warnings]
    const reason = [
      '[jt-ai-hook] AI checks failed.',
      '',
      ...reported.flatMap((outcome, index) => [
        ...(index > 0 ? [''] : []),
        section(outcome),
      ]),
    ].join('\n')
    if (runtime.input.stop_hook_active === true) {
      if (identity)
        runtime.clearTurnState(identity)
      writeOutput({
        continue: true,
        systemMessage: `${reason}\n\nStop hook already continued this turn; allowing stop to prevent an infinite loop.`,
      })
    }
    else {
      writeOutput({ decision: 'block', reason })
    }
    return
  }

  if (identity)
    runtime.clearTurnState(identity)
  if (warnings.length > 0) {
    writeOutput({
      continue: true,
      systemMessage: [
        '[jt-ai-hook] Some AI checks were skipped.',
        '',
        ...warnings.flatMap((outcome, index) => [
          ...(index > 0 ? [''] : []),
          section(outcome),
        ]),
      ].join('\n'),
    })
  }
  else {
    writeOutput({ continue: true, suppressOutput: true })
  }
})
