// jt-ai-hook

import { createHash } from 'node:crypto'
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'

import { resolveGitRoot } from './files'
import { readInput, writeOutput } from './protocol'

export type HookEventName = 'PostToolUse' | 'PreToolUse' | 'Stop'

export interface Fingerprint {
  exists: boolean
  hash: string | null
}

export interface StateIdentity {
  toolKey: string | null
  turnKey: string
}

const MAX_LOG_BYTES = 2 * 1024 * 1024
const STATE_TTL_MS = 24 * 60 * 60 * 1000

function hashKey(value: string): string {
  return createHash('sha256').update(value).digest('hex').slice(0, 24)
}

export class RuntimeContext {
  readonly cwd: string
  readonly input: Record<string, unknown>
  readonly inputCwd: string
  readonly logPath: string
  private readonly startedAt = Date.now()
  private readonly stateDir: string

  constructor(input: Record<string, unknown>) {
    this.input = input
    const unresolvedInputCwd = typeof input.cwd === 'string'
      ? resolve(input.cwd)
      : process.cwd()
    this.inputCwd = existsSync(unresolvedInputCwd)
      ? realpathSync(unresolvedInputCwd)
      : unresolvedInputCwd
    this.cwd = resolveGitRoot(this.inputCwd)

    const repoId = hashKey(this.cwd).slice(0, 12)
    this.stateDir = join('/tmp', `jt-ai-hook-state-${repoId}`)
    const configuredLog = process.env.JT_AI_HOOK_LOG || process.env.JT_VITEST_AI_HOOK_LOG
    this.logPath = configuredLog
      ? resolve(this.cwd, configuredLog)
      : join('/tmp', `jt-ai-hook-${repoId}.jsonl`)
  }

  writeLog(status: string, details: Record<string, unknown> = {}): void {
    try {
      mkdirSync(dirname(this.logPath), { recursive: true })
      if (existsSync(this.logPath) && statSync(this.logPath).size >= MAX_LOG_BYTES)
        writeFileSync(this.logPath, '')
      appendFileSync(this.logPath, `${JSON.stringify({
        durationMs: Date.now() - this.startedAt,
        hookEventName: typeof this.input.hook_event_name === 'string' ? this.input.hook_event_name : null,
        sessionId: typeof this.input.session_id === 'string' ? this.input.session_id : null,
        status,
        stopHookActive: this.input.stop_hook_active === true,
        timestamp: new Date().toISOString(),
        toolUseId: typeof this.input.tool_use_id === 'string' ? this.input.tool_use_id : null,
        turnId: typeof this.input.turn_id === 'string' ? this.input.turn_id : null,
        ...details,
      })}\n`)
    }
    catch {
      // Logging must never break hook output.
    }
  }

  stateIdentity(requireToolUse: boolean): StateIdentity | null {
    const sessionId = typeof this.input.session_id === 'string' ? this.input.session_id : null
    const turnId = typeof this.input.turn_id === 'string' ? this.input.turn_id : null
    const toolUseId = typeof this.input.tool_use_id === 'string' ? this.input.tool_use_id : null
    if (!sessionId || !turnId || (requireToolUse && !toolUseId))
      return null
    return {
      toolKey: toolUseId ? hashKey(toolUseId) : null,
      turnKey: hashKey(`${sessionId}\0${turnId}`),
    }
  }

  snapshotPath(identity: StateIdentity): string {
    return join(this.stateDir, `snapshot-${identity.turnKey}-${identity.toolKey}.json`)
  }

  recordPrefix(identity: StateIdentity): string {
    return `record-${identity.turnKey}-`
  }

  recordPath(identity: StateIdentity): string {
    return join(this.stateDir, `${this.recordPrefix(identity)}${identity.toolKey}.json`)
  }

  readState(path: string): unknown {
    try {
      return JSON.parse(readFileSync(path, 'utf8'))
    }
    catch {
      return null
    }
  }

  writeState(path: string, value: unknown): void {
    mkdirSync(this.stateDir, { recursive: true })
    writeFileSync(path, JSON.stringify(value))
  }

  removeState(path: string): void {
    try {
      rmSync(path, { force: true })
    }
    catch {
      // Cleanup failure must not break hook output.
    }
  }

  stateFiles(): string[] {
    try {
      return readdirSync(this.stateDir)
    }
    catch {
      return []
    }
  }

  statePath(name: string): string {
    return join(this.stateDir, name)
  }

  cleanupExpiredState(): void {
    const cutoff = Date.now() - STATE_TTL_MS
    for (const name of this.stateFiles()) {
      const path = this.statePath(name)
      try {
        if (statSync(path).mtimeMs < cutoff)
          this.removeState(path)
      }
      catch {
        this.removeState(path)
      }
    }
  }

  clearTurnState(identity: StateIdentity): void {
    for (const name of this.stateFiles()) {
      if (
        name.startsWith(this.recordPrefix(identity))
        || name.startsWith(`snapshot-${identity.turnKey}-`)
      ) {
        this.removeState(this.statePath(name))
      }
    }
  }

  continueSilently(): void {
    writeOutput({ continue: true, suppressOutput: true })
  }
}

export function runStage(
  event: HookEventName,
  handler: (runtime: RuntimeContext) => Promise<void> | void,
): void {
  void (async () => {
    if (process.argv[2] !== 'codex') {
      writeOutput({
        continue: true,
        systemMessage: '[jt-ai-hook] Missing or unsupported AI client id; validation was skipped.',
      })
      return
    }

    let runtime: RuntimeContext | null = null
    try {
      runtime = new RuntimeContext(readInput())
      runtime.cleanupExpiredState()
      if (runtime.input.hook_event_name !== event) {
        runtime.writeLog('skipped-unexpected-event', { expectedEvent: event })
        writeOutput({ continue: true })
        return
      }
      await handler(runtime)
    }
    catch (error) {
      const detail = error instanceof Error ? error.stack || error.message : String(error)
      runtime?.writeLog('hook-runtime-error', { error: detail.slice(0, 4000) })
      writeOutput({
        continue: true,
        systemMessage: `[jt-ai-hook] Hook runtime error; validation was skipped.${runtime ? ` Log: ${runtime.logPath}` : ''}`,
      })
    }
  })()
}
