export type AiHookStatus = "failed" | "passed" | "pending" | "skipped" | "warning"

export interface DataWarning {
  line?: number
  message: string
  sourcePath: string
}

export interface HookLogEntry {
  details?: Record<string, unknown>
  durationMs: number | null
  files: string[]
  hookEventName: string | null
  line: number
  message: string | null
  runner: string | null
  sessionId: string
  sourcePath: string
  status: string
  stopHookActive: boolean
  timestamp: string
  toolUseId: string | null
  turnId: string | null
}

export interface SessionMessage {
  line: number
  markdown: string
  role: "assistant" | "user"
  sourcePath: string
  timestamp: string
}

export interface SessionPatch {
  files: string[]
  line: number
  patch: string
  sourcePath: string
  timestamp: string
  turnId: string | null
}

export interface SessionDocument {
  context: string
  cwd: string | null
  id: string
  messages: SessionMessage[]
  patches: SessionPatch[]
  sourcePaths: string[]
  startedAt: string | null
  title: string
}

export interface AiHookSession extends SessionDocument {
  checkCount: number
  editCount: number
  entries: HookLogEntry[]
  fileCount: number
  lastExecutedAt: string | null
  logPaths: string[]
  status: AiHookStatus
  triggerCount: number
}

export interface AiHookDataset {
  loadedAt: string
  sessions: AiHookSession[]
  warnings: DataWarning[]
}

export interface AiHookLoadOptions {
  codexHome?: string
  logDirectory?: string
}
