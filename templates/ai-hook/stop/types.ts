// jt-ai-hook

export interface StopRunnerContext {
  cwd: string
  files: string[]
  logPath: string
  relativeFiles: string[]
}

export type StopRunnerStatus = 'failed' | 'passed' | 'warning'

export interface StopRunnerResult {
  details?: Record<string, unknown>
  message?: string
  status: StopRunnerStatus
}

export interface StopRunnerModule {
  run: (context: StopRunnerContext) => Promise<StopRunnerResult>
}
