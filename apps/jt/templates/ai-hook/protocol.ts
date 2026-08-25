// jt-ai-hook

import { readSync } from 'node:fs'
import process from 'node:process'

const MAX_INPUT_BYTES = 64 * 1024

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

export function readInput(): Record<string, unknown> {
  try {
    const chunks: Buffer[] = []
    let total = 0
    while (total <= MAX_INPUT_BYTES) {
      const chunk = Buffer.allocUnsafe(Math.min(8192, MAX_INPUT_BYTES + 1 - total))
      const read = readSync(0, chunk, 0, chunk.length, null)
      if (read === 0)
        break
      chunks.push(chunk.subarray(0, read))
      total += read
    }
    if (total > MAX_INPUT_BYTES)
      return {}
    const raw = Buffer.concat(chunks).toString('utf8')
    const value: unknown = raw.trim() ? JSON.parse(raw) : {}
    return isRecord(value) ? value : {}
  }
  catch {
    return {}
  }
}

export function writeOutput(value: Record<string, unknown>): void {
  process.stdout.write(JSON.stringify(value))
}
