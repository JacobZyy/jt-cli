// jt-ai-hook

import type { StopRunnerContext, StopRunnerResult } from '../types'
import { existsSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join, relative } from 'node:path'
import process from 'node:process'

import { isRecord } from '../../protocol'
import { runProcess } from '../process'

interface EslintMessage {
  column?: unknown
  line?: unknown
  message?: unknown
  ruleId?: unknown
  severity?: unknown
}

interface EslintResult {
  errorCount?: unknown
  filePath?: unknown
  messages?: unknown
}

const MAX_DIAGNOSTICS = 50
const SUPPORTED_FILE = /\.(?:[cm]?[jt]sx?|vue|svelte|astro|jsonc?|ya?ml|toml|md|mdx)$/i

function resolveEslint(cwd: string): string | null {
  try {
    const requireFromProject = createRequire(join(cwd, 'package.json'))
    const packageJson = requireFromProject.resolve('eslint/package.json')
    return join(dirname(packageJson), 'bin/eslint.js')
  }
  catch {
    const fallback = join(cwd, 'node_modules/eslint/bin/eslint.js')
    return existsSync(fallback) ? fallback : null
  }
}

function parseResults(stdout: unknown): EslintResult[] | null {
  const lines = String(stdout || '').split(/\r?\n/)
  for (let index = 0; index < lines.length; index++) {
    const candidate = lines.slice(index).join('\n').trim()
    if (!candidate)
      continue
    try {
      const value: unknown = JSON.parse(candidate)
      if (Array.isArray(value))
        return value.filter(isRecord)
    }
    catch {
      // Config loading may print notices before formatter JSON.
    }
  }
  return null
}

function formatDiagnostics(cwd: string, results: EslintResult[]): {
  diagnostics: string[]
  errorCount: number
} {
  const diagnostics: string[] = []
  let errorCount = 0

  for (const result of results) {
    const count = Number(result.errorCount || 0)
    if (Number.isFinite(count))
      errorCount += count
    const messages = Array.isArray(result.messages) ? result.messages.filter(isRecord) : []
    for (const message of messages as EslintMessage[]) {
      if (message.severity !== 2 || diagnostics.length >= MAX_DIAGNOSTICS)
        continue
      const resultFile = typeof result.filePath === 'string' ? result.filePath : ''
      const file = relative(cwd, resultFile) || resultFile || '<unknown>'
      const location = `${String(message.line || 1)}:${String(message.column || 1)}`
      const rule = typeof message.ruleId === 'string' && message.ruleId ? ` [${message.ruleId}]` : ''
      diagnostics.push(`${file}:${location} error${rule} ${String(message.message || '')}`)
    }
  }

  const omitted = Math.max(0, errorCount - diagnostics.length)
  if (omitted > 0)
    diagnostics.push(`... ${omitted} additional diagnostic(s) omitted`)
  return { diagnostics, errorCount }
}

function boundedOutput(stdout: unknown, stderr: unknown): string {
  const output = [stdout, stderr]
    .map(value => String(value || '').trim())
    .filter(Boolean)
    .join('\n')
  return output.length <= 4_000
    ? output
    : `${output.slice(0, 4_000)}\n... ESLint output truncated`
}

export async function run(context: StopRunnerContext): Promise<StopRunnerResult> {
  const files = context.files.filter((file) => {
    try {
      return SUPPORTED_FILE.test(file) && existsSync(file) && statSync(file).isFile()
    }
    catch {
      return false
    }
  })
  if (files.length === 0)
    return { status: 'passed' }

  const eslintBin = resolveEslint(context.cwd)
  if (!eslintBin) {
    return {
      status: 'warning',
      message: `ESLint not found. Install repository dependencies before relying on AI-hook diagnostics. Log: ${context.logPath}`,
    }
  }

  const result = await runProcess(
    process.execPath,
    [eslintBin, ...files, '--format', 'json', '--no-warn-ignored'],
    {
      cwd: context.cwd,
      env: { ...process.env, isInAIHook: 'true', NO_COLOR: '1' },
      maxBuffer: 10 * 1024 * 1024,
      timeout: 120_000,
    },
  )
  const baseDetails = {
    exitCode: result.status,
    files: files.map(file => relative(context.cwd, file)),
    signal: result.signal,
  }
  if (result.error || result.status === null || result.status > 1) {
    const detail = boundedOutput(result.stderr, result.error)
      || 'unknown ESLint execution failure'
    return {
      details: { ...baseDetails, output: detail },
      status: 'failed',
      message: `ESLint could not complete. Fix lint configuration/runtime, then rerun validation.\nLog: ${context.logPath}\n${detail}`,
    }
  }

  const parsed = parseResults(result.stdout)
  if (!parsed) {
    const detail = boundedOutput(result.stdout, result.stderr)
    return {
      details: { ...baseDetails, output: detail },
      status: 'failed',
      message: `ESLint returned invalid JSON output.\nLog: ${context.logPath}\n${detail}`,
    }
  }

  const summary = formatDiagnostics(context.cwd, parsed)
  const details = { ...baseDetails, ...summary }
  if (summary.errorCount === 0)
    return { details, status: 'passed' }

  return {
    details,
    status: 'failed',
    message: [
      `ESLint found ${summary.errorCount} error(s) in AI-edited files.`,
      'Fix these diagnostics. Do not bypass rules or run broad --fix unless requested.',
      `Log: ${context.logPath}`,
      '',
      ...summary.diagnostics,
    ].join('\n'),
  }
}
