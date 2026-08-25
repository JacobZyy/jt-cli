import { basename } from "node:path"

import { z } from "zod"

import type {
  AiHookSession,
  AiHookStatus,
  DataWarning,
  HookLogEntry,
  SessionDocument,
  SessionMessage,
  SessionPatch,
} from "./types"

const hookLogSchema = z.object({
  details: z.record(z.string(), z.unknown()).optional(),
  durationMs: z.number().optional(),
  files: z.array(z.string()).optional(),
  hookEventName: z.string().nullable().optional(),
  message: z.string().nullable().optional(),
  runner: z.string().nullable().optional(),
  sessionId: z.string().nullable().optional(),
  status: z.string(),
  stopHookActive: z.boolean().optional(),
  timestamp: z.string(),
  toolUseId: z.string().nullable().optional(),
  turnId: z.string().nullable().optional(),
})

const sessionMetaSchema = z.object({
  type: z.literal("session_meta"),
  payload: z.object({
    cwd: z.string().optional(),
    id: z.string().optional(),
    session_id: z.string().optional(),
    timestamp: z.string().optional(),
  }),
})

const responseItemSchema = z.object({
  timestamp: z.string(),
  type: z.literal("response_item"),
  payload: z.object({
    content: z.array(z.unknown()).optional(),
    input: z.string().optional(),
    internal_chat_message_metadata_passthrough: z.object({
      turn_id: z.string().optional(),
    }).optional(),
    name: z.string().optional(),
    role: z.string().optional(),
    type: z.string(),
  }).passthrough(),
})

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter(item => typeof item === "string") : []
}

function lineFiles(value: z.infer<typeof hookLogSchema>): string[] {
  return [...new Set([
    ...strings(value.files),
    ...strings(value.details?.files),
  ])]
}

export function parseHookLog(content: string, sourcePath: string) {
  const entries: HookLogEntry[] = []
  const warnings: DataWarning[] = []

  content.split("\n").forEach((line, index) => {
    if (!line.trim()) return
    try {
      const value = hookLogSchema.parse(JSON.parse(line))
      entries.push({
        details: value.details,
        durationMs: value.durationMs ?? null,
        files: lineFiles(value),
        hookEventName: value.hookEventName ?? null,
        line: index + 1,
        message: value.message ?? null,
        runner: value.runner ?? null,
        sessionId: value.sessionId || `unknown:${basename(sourcePath)}`,
        sourcePath,
        status: value.status,
        stopHookActive: value.stopHookActive ?? false,
        timestamp: value.timestamp,
        toolUseId: value.toolUseId ?? null,
        turnId: value.turnId ?? null,
      })
    }
    catch (error) {
      warnings.push({
        line: index + 1,
        message: error instanceof Error ? error.message : String(error),
        sourcePath,
      })
    }
  })

  return { entries, warnings }
}

function textContent(content: unknown[] | undefined): string {
  if (!content) return ""
  return content.flatMap((item) => {
    if (!item || typeof item !== "object") return []
    const value = item as Record<string, unknown>
    for (const field of ["text", "input_text", "output_text"]) {
      if (typeof value[field] === "string") return [value[field]]
    }
    return []
  }).join("\n")
}

function meaningfulUserText(markdown: string): string {
  const environmentBoundary = markdown.lastIndexOf("</environment_context>")
  const withoutEnvironment = environmentBoundary >= 0
    ? markdown.slice(environmentBoundary + "</environment_context>".length).trim()
    : markdown
    .replace(/<recommended_plugins>[\s\S]*?<\/recommended_plugins>/g, "")
    .replace(/<environment_context>[\s\S]*?<\/environment_context>/g, "")
    .trim()
  const requestBoundary = withoutEnvironment.lastIndexOf("## My request:")
  return requestBoundary >= 0
    ? withoutEnvironment.slice(requestBoundary + "## My request:".length).trim()
    : withoutEnvironment
}

function titleFromContext(context: string, sessionId: string): string {
  const firstLine = context
    .split("\n")
    .map(line => line
      .replace(/^#+\s*/, "")
      .replace(/\[([^\]]+)]\([^)]+\)/g, "$1")
      .replace(/[`*_~]/g, "")
      .trim())
    .find(Boolean)
  if (!firstLine) return `会话 ${sessionId.slice(0, 8)}`
  return firstLine.length > 64 ? `${firstLine.slice(0, 63)}…` : firstLine
}

function decodePatch(rawPatch: string): string {
  try {
    return JSON.parse(`"${rawPatch}"`) as string
  }
  catch {
    return rawPatch
      .replace(/\\\\n/g, "\n")
      .replace(/\\n/g, "\n")
      .replace(/\\"/g, "\"")
  }
}

function patchFromInput(input: string | undefined): string | null {
  if (!input) return null
  const start = input.indexOf("*** Begin Patch")
  const end = input.indexOf("*** End Patch", start)
  if (start < 0 || end < 0) return null
  return decodePatch(input.slice(start, end + "*** End Patch".length))
}

function patchFiles(patch: string): string[] {
  return [...patch.matchAll(/^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm)]
    .map(match => match[1]?.trim())
    .filter((file): file is string => Boolean(file))
}

export function parseSessionDocument(
  contents: Array<{ content: string, sourcePath: string }>,
  fallbackId: string,
) {
  const warnings: DataWarning[] = []
  const messages: SessionMessage[] = []
  const patches: SessionPatch[] = []
  let cwd: string | null = null
  let id = fallbackId
  let startedAt: string | null = null

  for (const { content, sourcePath } of contents) {
    content.split("\n").forEach((line, index) => {
      if (!line.trim()) return
      let raw: unknown
      try {
        raw = JSON.parse(line)
      }
      catch (error) {
        warnings.push({
          line: index + 1,
          message: error instanceof Error ? error.message : String(error),
          sourcePath,
        })
        return
      }

      const meta = sessionMetaSchema.safeParse(raw)
      if (meta.success) {
        id = meta.data.payload.id || meta.data.payload.session_id || id
        cwd = meta.data.payload.cwd ?? cwd
        startedAt = meta.data.payload.timestamp ?? startedAt
        return
      }

      const item = responseItemSchema.safeParse(raw)
      if (!item.success) return
      const { payload, timestamp } = item.data
      if (payload.type === "message" && (payload.role === "user" || payload.role === "assistant")) {
        const rawMarkdown = textContent(payload.content)
        const markdown = payload.role === "user" ? meaningfulUserText(rawMarkdown) : rawMarkdown.trim()
        if (markdown) {
          messages.push({
            line: index + 1,
            markdown,
            role: payload.role,
            sourcePath,
            timestamp,
          })
        }
      }

      if (
        (payload.type === "custom_tool_call" || payload.type === "function_call")
        && (payload.name === "exec" || payload.name === "apply_patch")
      ) {
        const patch = patchFromInput(payload.input)
        if (patch) {
          patches.push({
            files: patchFiles(patch),
            line: index + 1,
            patch,
            sourcePath,
            timestamp,
            turnId: payload.internal_chat_message_metadata_passthrough?.turn_id ?? null,
          })
        }
      }
    })
  }

  messages.sort((left, right) => left.timestamp.localeCompare(right.timestamp))
  patches.sort((left, right) => left.timestamp.localeCompare(right.timestamp))
  const context = messages.find(message => message.role === "user")?.markdown ?? ""
  const document: SessionDocument = {
    context,
    cwd,
    id,
    messages,
    patches,
    sourcePaths: contents.map(item => item.sourcePath),
    startedAt,
    title: titleFromContext(context, id),
  }
  return { document, warnings }
}

function statusFromEntries(entries: HookLogEntry[]): AiHookStatus {
  const stopEntries = entries.filter(entry => entry.hookEventName === "Stop")
  if (stopEntries.length === 0) return entries.length > 0 ? "pending" : "skipped"

  const latest = stopEntries.findLast(entry => (
    entry.status.startsWith("runner-") || entry.status === "hook-runtime-error"
  )) ?? stopEntries.at(-1)!
  const finalBatch = stopEntries.filter(entry => (
    entry.turnId === latest.turnId
    && entry.stopHookActive === latest.stopHookActive
    && Math.abs(Date.parse(entry.timestamp) - Date.parse(latest.timestamp)) < 5_000
  ))
  const statuses = finalBatch.map(entry => entry.status)
  if (statuses.some(status => status === "runner-failed" || status === "hook-runtime-error")) return "failed"
  if (statuses.some(status => status.includes("warning") || status === "runner-warning")) return "warning"
  if (statuses.some(status => status === "runner-passed")) return "passed"
  if (statuses.every(status => status.startsWith("skipped-"))) return "skipped"
  return "pending"
}

function checkCount(entries: HookLogEntry[]): number {
  return new Set(entries
    .filter(entry => entry.hookEventName === "Stop")
    .map(entry => `${entry.turnId ?? entry.timestamp}:${entry.stopHookActive}`)).size
}

export function createAiHookSession(
  entries: HookLogEntry[],
  document: SessionDocument,
): AiHookSession {
  const sortedEntries = entries.toSorted((left, right) => left.timestamp.localeCompare(right.timestamp))
  return {
    ...document,
    checkCount: checkCount(sortedEntries),
    editCount: sortedEntries.filter(entry => entry.status === "post-recorded-edits").length,
    entries: sortedEntries,
    fileCount: new Set(sortedEntries.flatMap(entry => entry.files)).size,
    lastExecutedAt: sortedEntries.at(-1)?.timestamp ?? null,
    logPaths: [...new Set(sortedEntries.map(entry => entry.sourcePath))],
    status: statusFromEntries(sortedEntries),
    triggerCount: sortedEntries.length,
  }
}
