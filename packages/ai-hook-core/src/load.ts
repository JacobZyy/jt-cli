import { readFile, readdir } from "node:fs/promises"
import { homedir } from "node:os"
import { basename, isAbsolute, join, resolve } from "node:path"

import { createAiHookSession, parseHookLog, parseSessionDocument } from "./parse"
import type {
  AiHookDataset,
  AiHookLoadOptions,
  DataWarning,
  HookLogEntry,
} from "./types"

const LOG_FILE = /^jt-ai-hook-[0-9a-f]{12}\.jsonl$/
const DEFAULT_LOG_DIRECTORY = "/tmp"

function configuredDirectory(value: string | undefined, fallback: string): string {
  if (!value) return fallback
  return isAbsolute(value) ? value : resolve(value)
}

async function filesUnder(directory: string, warnings: DataWarning[]): Promise<string[]> {
  try {
    const names = await readdir(directory, { recursive: true })
    return names.map(name => join(directory, name))
  }
  catch (error) {
    warnings.push({
      message: error instanceof Error ? error.message : String(error),
      sourcePath: directory,
    })
    return []
  }
}

async function filesIn(directory: string, warnings: DataWarning[]): Promise<string[]> {
  try {
    return (await readdir(directory)).map(name => join(directory, name))
  }
  catch (error) {
    warnings.push({
      message: error instanceof Error ? error.message : String(error),
      sourcePath: directory,
    })
    return []
  }
}

async function readText(path: string, warnings: DataWarning[]): Promise<string | null> {
  try {
    return await readFile(path, "utf8")
  }
  catch (error) {
    warnings.push({
      message: error instanceof Error ? error.message : String(error),
      sourcePath: path,
    })
    return null
  }
}

function entriesBySession(entries: HookLogEntry[]): Map<string, HookLogEntry[]> {
  const grouped = new Map<string, HookLogEntry[]>()
  for (const entry of entries) {
    const current = grouped.get(entry.sessionId) ?? []
    current.push(entry)
    grouped.set(entry.sessionId, current)
  }
  return grouped
}

export async function loadAiHookData(options: AiHookLoadOptions = {}): Promise<AiHookDataset> {
  const warnings: DataWarning[] = []
  const codexHome = configuredDirectory(
    options.codexHome || process.env.CODEX_HOME,
    join(homedir(), ".codex"),
  )
  const logDirectory = configuredDirectory(
    options.logDirectory || process.env.AI_HOOK_LOG_DIR,
    DEFAULT_LOG_DIRECTORY,
  )

  const logPaths = (await filesIn(logDirectory, warnings))
    .filter(path => LOG_FILE.test(basename(path)))
    .toSorted()
  const logContents = await Promise.all(logPaths.map(async sourcePath => ({
    content: await readText(sourcePath, warnings),
    sourcePath,
  })))
  const entries: HookLogEntry[] = []
  for (const { content, sourcePath } of logContents) {
    if (content === null) continue
    const parsed = parseHookLog(content, sourcePath)
    entries.push(...parsed.entries)
    warnings.push(...parsed.warnings)
  }

  const sessionPaths = [
    ...(await filesUnder(join(codexHome, "sessions"), warnings)),
    ...(await filesUnder(join(codexHome, "archived_sessions"), warnings)),
  ].filter(path => path.endsWith(".jsonl"))
  const grouped = entriesBySession(entries)
  const sessions = await Promise.all([...grouped].map(async ([sessionId, sessionEntries]) => {
    const matchedPaths = sessionPaths.filter(path => basename(path).includes(sessionId)).toSorted()
    const contents = (await Promise.all(matchedPaths.map(async sourcePath => ({
      content: await readText(sourcePath, warnings),
      sourcePath,
    })))).flatMap(item => item.content === null ? [] : [{
      content: item.content,
      sourcePath: item.sourcePath,
    }])
    const parsed = parseSessionDocument(contents, sessionId)
    warnings.push(...parsed.warnings)
    return createAiHookSession(sessionEntries, {
      ...parsed.document,
      id: sessionId,
    })
  }))

  return {
    loadedAt: new Date().toISOString(),
    sessions: sessions.toSorted((left, right) => (
      (right.lastExecutedAt ?? "").localeCompare(left.lastExecutedAt ?? "")
    )),
    warnings,
  }
}

export async function loadAiHookSession(
  sessionId: string,
  options: AiHookLoadOptions = {},
) {
  const data = await loadAiHookData(options)
  return {
    session: data.sessions.find(item => item.id === sessionId) ?? null,
    warnings: data.warnings,
  }
}
