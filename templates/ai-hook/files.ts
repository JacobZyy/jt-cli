// jt-ai-hook

import type { Fingerprint } from './runtime'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, realpathSync, statSync } from 'node:fs'
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path'

export function resolveGitRoot(cwd: string): string {
  try {
    return realpathSync(execFileSync('git', ['rev-parse', '--show-toplevel'], {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
      timeout: 10_000,
    }).trim())
  }
  catch {
    return resolve(cwd)
  }
}

function isWithinRoot(root: string, file: string): boolean {
  const pathFromRoot = relative(root, file)
  return pathFromRoot !== ''
    && pathFromRoot !== '..'
    && !pathFromRoot.startsWith(`..${sep}`)
    && !isAbsolute(pathFromRoot)
}

function isWithinOrEqualRoot(root: string, file: string): boolean {
  return file === root || isWithinRoot(root, file)
}

function nearestExistingPath(file: string): string | null {
  let current = file
  while (!existsSync(current)) {
    const parent = dirname(current)
    if (parent === current)
      return null
    current = parent
  }
  return current
}

export function safeFilePath(root: string, candidate: unknown, base = root): string | null {
  if (typeof candidate !== 'string' || candidate.includes('\0'))
    return null

  const file = isAbsolute(candidate) ? resolve(candidate) : resolve(base, candidate)
  if (!isWithinRoot(root, file))
    return null

  try {
    const existingPath = nearestExistingPath(file)
    if (!existingPath || !isWithinOrEqualRoot(root, realpathSync(existingPath)))
      return null
  }
  catch {
    return null
  }
  return file
}

export function fingerprint(file: string): Fingerprint {
  try {
    if (!existsSync(file) || !statSync(file).isFile())
      return { exists: false, hash: null }
    return {
      exists: true,
      hash: createHash('sha256').update(readFileSync(file)).digest('hex'),
    }
  }
  catch {
    return { exists: existsSync(file), hash: null }
  }
}

function parsePatchFiles(command: unknown): string[] {
  if (typeof command !== 'string')
    return []

  const files = new Set<string>()
  for (const pattern of [
    /^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm,
    /^\*\*\* Move to: (.+)$/gm,
  ]) {
    for (const match of command.matchAll(pattern)) {
      const file = match[1]?.trim()
      if (file)
        files.add(file)
    }
  }
  return [...files]
}

export function extractCandidates(toolInput: Record<string, unknown>): string[] {
  return parsePatchFiles(toolInput.command)
}
