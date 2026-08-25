// jt-ai-hook

import { spawn } from 'node:child_process'

export interface ProcessResult {
  error: string | null
  signal: string | null
  status: number | null
  stderr: string
  stdout: string
  timedOut: boolean
}

interface ProcessOptions {
  cwd: string
  env: NodeJS.ProcessEnv
  maxBuffer: number
  timeout: number
}

interface Capture {
  chunks: Buffer[]
  size: number
  truncated: boolean
}

function append(capture: Capture, chunk: Buffer, limit: number): void {
  const remaining = limit - capture.size
  if (remaining <= 0) {
    capture.truncated = true
    return
  }
  const selected = chunk.length > remaining ? chunk.subarray(0, remaining) : chunk
  capture.chunks.push(selected)
  capture.size += selected.length
  capture.truncated ||= selected.length < chunk.length
}

function text(capture: Capture): string {
  const value = Buffer.concat(capture.chunks).toString('utf8')
  return capture.truncated ? `${value}\n... output truncated` : value
}

export function runProcess(
  command: string,
  args: string[],
  options: ProcessOptions,
): Promise<ProcessResult> {
  return new Promise((resolve) => {
    const stdout: Capture = { chunks: [], size: 0, truncated: false }
    const stderr: Capture = { chunks: [], size: 0, truncated: false }
    let error: string | null = null
    let timedOut = false
    let killTimer: NodeJS.Timeout | null = null
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    child.stdout.on('data', (chunk: Buffer) => append(stdout, chunk, options.maxBuffer))
    child.stderr.on('data', (chunk: Buffer) => append(stderr, chunk, options.maxBuffer))
    child.on('error', (failure) => {
      error = failure.message
    })

    const timeout = setTimeout(() => {
      timedOut = true
      error = `process timed out after ${options.timeout}ms`
      child.kill('SIGTERM')
      killTimer = setTimeout(() => child.kill('SIGKILL'), 1_000)
    }, options.timeout)

    child.on('close', (status, signal) => {
      clearTimeout(timeout)
      if (killTimer)
        clearTimeout(killTimer)
      resolve({
        error,
        signal,
        status,
        stderr: text(stderr),
        stdout: text(stdout),
        timedOut,
      })
    })
  })
}
