// jt-ai-hook

import type { ProcessResult } from '../process'
import type { StopRunnerContext, StopRunnerResult } from '../types'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join, relative, sep } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

import { isRecord } from '../../protocol'
import { runProcess } from '../process'

const COVERAGE_OUTPUT_MARKER = '__JT_VITEST_COVERAGE_FILES__'
const COVERAGE_SUMMARY_FILE = 'coverage-summary.json'

type CoverageValue = number | 'Unknown'

interface CoverageSelection {
  excludedFiles: string[]
  files: string[]
  skipFull: boolean
  warning: string | null
}

interface CoverageRow {
  branches: CoverageValue
  file: string
  functions: CoverageValue
  lines: CoverageValue
  statements: CoverageValue
}

interface VitestExecution {
  coverageRows: CoverageRow[]
  coverageWarning: string | null
  result: ProcessResult
}

function resolveVitest(cwd: string): string | null {
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

async function executeVitest(
  vitestBin: string,
  cwd: string,
  files: string[],
  coverageFiles: string[],
  skipFull: boolean,
): Promise<VitestExecution> {
  const reportsDirectory = coverageFiles.length > 0
    ? mkdtempSync(join(tmpdir(), 'jt-vitest-coverage-'))
    : null
  const coverageArgs = coverageFiles.length > 0
    ? [
        '--coverage.enabled',
        ...coverageFiles.map(file =>
          `--coverage.include=${relative(cwd, file).split(sep).join('/')}`,
        ),
        '--coverage.reporter=json-summary',
        `--coverage.reportsDirectory=${reportsDirectory}`,
        '--coverage.reportOnFailure',
      ]
    : ['--coverage.enabled=false']

  try {
    const result = await runProcess(
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
        env: { ...process.env, isInAIHook: 'true', NO_COLOR: '1' },
        maxBuffer: 10 * 1024 * 1024,
        timeout: 120_000,
      },
    )
    if (!reportsDirectory) {
      return {
        coverageRows: [],
        coverageWarning: null,
        result,
      }
    }

    const summaryFile = join(reportsDirectory, COVERAGE_SUMMARY_FILE)
    if (!existsSync(summaryFile)) {
      return {
        coverageRows: [],
        coverageWarning: 'Structured coverage summary was not generated.',
        result,
      }
    }
    try {
      return {
        coverageRows: readCoverageRows(cwd, summaryFile, skipFull),
        coverageWarning: null,
        result,
      }
    }
    catch (error) {
      const detail = error instanceof Error ? error.message : String(error)
      return {
        coverageRows: [],
        coverageWarning: `Could not read structured coverage summary. ${detail.slice(0, 1000)}`,
        result,
      }
    }
  }
  finally {
    if (reportsDirectory)
      rmSync(reportsDirectory, { force: true, recursive: true })
  }
}

function coverageValue(summary: Record<string, unknown>, key: string): CoverageValue | null {
  const metric = summary[key]
  if (!isRecord(metric))
    return null
  return typeof metric.pct === 'number' && Number.isFinite(metric.pct)
    ? metric.pct
    : metric.pct === 'Unknown'
      ? metric.pct
      : null
}

function coverageRow(file: string, value: unknown): CoverageRow | null {
  if (!isRecord(value))
    return null
  const branches = coverageValue(value, 'branches')
  const functions = coverageValue(value, 'functions')
  const lines = coverageValue(value, 'lines')
  const statements = coverageValue(value, 'statements')
  return branches === null || functions === null || lines === null || statements === null
    ? null
    : { branches, file, functions, lines, statements }
}

function readCoverageRows(cwd: string, summaryFile: string, skipFull: boolean): CoverageRow[] {
  const summary: unknown = JSON.parse(readFileSync(summaryFile, 'utf8'))
  if (!isRecord(summary))
    throw new TypeError('Coverage summary must be a JSON object.')
  const total = coverageRow('All files', summary.total)
  if (!total)
    throw new TypeError('Coverage summary has no valid total.')

  const rows = Object.entries(summary)
    .filter(([file]) => file !== 'total')
    .map(([file, value]) => {
      const displayFile = relative(cwd, file).split(sep).join('/') || file
      const row = coverageRow(displayFile, value)
      if (!row)
        throw new TypeError(`Coverage summary has invalid metrics for ${displayFile}.`)
      return row
    })
    .filter(row => !skipFull || [row.statements, row.branches, row.functions, row.lines]
      .some(value => value !== 100))
    .sort((left, right) => left.file.localeCompare(right.file))
  return [total, ...rows]
}

function markdownCell(value: string): string {
  return value.replace(/[\r\n]+/g, ' ').replace(/\|/g, '\\|')
}

function percentage(value: CoverageValue): string {
  return typeof value === 'number' ? `${value}%` : value
}

function formatCoverageTable(rows: CoverageRow[]): string {
  if (rows.length === 0)
    return ''
  return [
    '| File | Statements | Branches | Functions | Lines |',
    '|---|---:|---:|---:|---:|',
    ...rows.map(row => `| ${markdownCell(row.file)} | ${percentage(row.statements)} | ${percentage(row.branches)} | ${percentage(row.functions)} | ${percentage(row.lines)} |`),
  ].join('\n')
}

function coverageThresholdFailures(stdout: unknown, stderr: unknown): string[] {
  const failures = new Set<string>()
  const output = [stdout, stderr].map(value => String(value || '')).join('\n')
  for (const rawLine of output.split('\n')) {
    const line = rawLine.trim()
    const percentageFailure = line.match(
      /^ERROR: Coverage for (\w+) \(([\d.]+)%\) does not meet (.+?) threshold \(([\d.]+)%\)(?: for (.+))?$/,
    )
    if (percentageFailure) {
      const [, metric, actual, scope, expected, file] = percentageFailure
      const label = scope === 'global' && !file
        ? `All files ${metric}`
        : `${file || scope} ${metric}`
      failures.add(`- ${label}: ${actual}% < ${expected}%`)
      continue
    }
    const uncoveredFailure = line.match(
      /^ERROR: Uncovered (\w+) \((\d+)\) exceed (.+?) threshold \((\d+)\)(?: for (.+))?$/,
    )
    if (uncoveredFailure) {
      const [, metric, actual, scope, expected, file] = uncoveredFailure
      const label = scope === 'global' && !file
        ? `All files uncovered ${metric}`
        : `${file || scope} uncovered ${metric}`
      failures.add(`- ${label}: ${actual} > ${expected}`)
    }
  }
  return [...failures]
}

function agentOutput(stdout: unknown): string {
  return String(stdout || '')
    .split('\n')
    .filter(line => !/^\s*Coverage enabled with .+\s*$/.test(line))
    .filter(line => !/^ERROR: (?:Coverage for|Uncovered )/.test(line.trim()))
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

function diagnosticOutput(stderr: unknown): string {
  return String(stderr || '')
    .split('\n')
    .filter(line => !/^ERROR: (?:Coverage for|Uncovered )/.test(line.trim()))
    .join('\n')
    .trim()
}

function allTestsPassed(output: string): boolean {
  return /^\s*Test Files\s+(\d+) passed \(\1\)\s*$/m.test(output)
    && /^\s*Tests\s+(\d+) passed \(\1\)\s*$/m.test(output)
}

function isCoverageOnlyFailure(execution: VitestExecution): boolean {
  return !execution.result.error
    && !execution.coverageWarning
    && execution.coverageRows.length > 0
    && allTestsPassed(agentOutput(execution.result.stdout))
    && coverageThresholdFailures(execution.result.stdout, execution.result.stderr).length > 0
}

function formatFailureReport(execution: VitestExecution): string {
  const tests = agentOutput(execution.result.stdout)
  const coverageOnly = isCoverageOnlyFailure(execution)
  const diagnostics = tests || execution.result.error
    ? ''
    : diagnosticOutput(execution.result.stderr)
  const coverage = formatCoverageTable(execution.coverageRows)
  const thresholds = coverageThresholdFailures(
    execution.result.stdout,
    execution.result.stderr,
  )
  const sections = [
    !coverageOnly && tests && `Tests\n${tests}`,
    coverage && `Coverage\n${coverage}`,
    thresholds.length > 0 && `Coverage thresholds\n${thresholds.join('\n')}`,
    execution.coverageWarning && `Coverage warning\n${execution.coverageWarning}`,
    execution.result.error && `Runtime\n${execution.result.error}`,
    diagnostics && `Diagnostics\n${diagnostics}`,
  ].filter((section): section is string => typeof section === 'string')
  if (sections.length === 0) {
    const fallback = boundedOutput(execution.result.stdout, execution.result.stderr)
    return fallback || 'Vitest exited without output.'
  }
  return boundedOutput(sections.join('\n\n'), '')
}

async function selectCoverageFiles(cwd: string, files: string[]): Promise<CoverageSelection> {
  const helper = fileURLToPath(new URL('../support/vitest-coverage.ts', import.meta.url))
  const result = await runProcess(
    'pnpm',
    ['--dir', cwd, 'exec', 'tsx', helper, cwd, ...files],
    {
      cwd,
      env: { ...process.env, isInAIHook: 'true', NO_COLOR: '1' },
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
        isRecord(parsed)
        && parsed.ok === true
        && Array.isArray(parsed.files)
      ) {
        const allowed = new Set(files)
        const selected = parsed.files
          .filter((file: unknown): file is string => typeof file === 'string' && allowed.has(file))
        const selectedSet = new Set(selected)
        return {
          excludedFiles: files.filter(file => !selectedSet.has(file)),
          files: selected,
          skipFull: parsed.skipFull === true,
          warning: null,
        }
      }
      const error = isRecord(parsed) ? parsed.error : null
      if (typeof error === 'string')
        throw new Error(error)
    }
    catch (error) {
      const detail = error instanceof Error ? error.message : String(error)
      return {
        excludedFiles: [],
        files,
        skipFull: false,
        warning: `Could not resolve project coverage filters; using all AI-edited files. ${detail}`,
      }
    }
  }

  const detail = boundedOutput(result.stdout, result.stderr).slice(0, 1000)
  return {
    excludedFiles: [],
    files,
    skipFull: false,
    warning: `Could not resolve project coverage filters; using all AI-edited files. ${detail || result.error || 'filter process failed'}`,
  }
}

function boundedOutput(stdout: unknown, stderr: unknown): string {
  const output = [stdout, stderr]
    .map(value => String(value || '').trim())
    .filter(Boolean)
    .join('\n')
  return output.length <= 12_000
    ? output
    : `${output.slice(0, 12_000)}\n... Vitest output truncated`
}

export async function run(context: StopRunnerContext): Promise<StopRunnerResult> {
  const vitestBin = resolveVitest(context.cwd)
  if (!vitestBin) {
    return {
      status: 'warning',
      message: `Vitest not found. Install repository dependencies before relying on AI-hook tests. Log: ${context.logPath}`,
    }
  }

  const coverage = await selectCoverageFiles(context.cwd, context.files)
  const execution = await executeVitest(
    vitestBin,
    context.cwd,
    context.files,
    coverage.files,
    coverage.skipFull,
  )
  const detail = boundedOutput(execution.result.stdout, execution.result.stderr)
  const details = {
    coverageExcludedFiles: coverage.excludedFiles.map(file => relative(context.cwd, file)),
    coverageFiles: coverage.files.map(file => relative(context.cwd, file)),
    coverageRows: execution.coverageRows,
    coverageWarning: execution.coverageWarning || coverage.warning,
    exitCode: execution.result.status,
    files: context.relativeFiles,
    output: detail.slice(0, 4000),
    signal: execution.result.signal,
  }
  if (execution.result.status === 0) {
    return coverage.warning
      ? { details, status: 'warning', message: `${coverage.warning} Log: ${context.logPath}` }
      : { details, status: 'passed' }
  }

  const report = formatFailureReport(execution)
  const coverageOnly = !coverage.warning && isCoverageOnlyFailure(execution)
  const message = coverageOnly
    ? report
    : [
        execution.result.error
          ? 'Vitest could not complete for AI-edited files.'
          : 'Related Vitest suites or coverage checks failed for AI-edited files.',
        `Changed files: ${context.relativeFiles.join(', ')}`,
        ...(coverage.warning ? [coverage.warning] : []),
        `Log: ${context.logPath}`,
        report,
      ].join('\n')
  return { details, status: 'failed', message }
}
