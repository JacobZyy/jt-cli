// jt-ai-hook
// Captures file fingerprints before an AI patch runs.

import { relative } from 'node:path'

import { extractCandidates, fingerprint, safeFilePath } from './files'
import { isRecord } from './protocol'
import { runStage } from './runtime'

runStage('PreToolUse', (runtime) => {
  const { input } = runtime
  if (input.tool_name !== 'apply_patch') {
    runtime.writeLog('pre-skipped-unsupported-tool', { toolName: input.tool_name || null })
    runtime.continueSilently()
    return
  }

  const identity = runtime.stateIdentity(true)
  if (!identity) {
    runtime.writeLog('pre-skipped-missing-identity')
    runtime.continueSilently()
    return
  }

  const toolInput = isRecord(input.tool_input) ? input.tool_input : {}
  const files = extractCandidates(toolInput)
    .map(file => safeFilePath(runtime.cwd, file, runtime.inputCwd))
    .filter((file): file is string => file !== null)

  if (files.length === 0) {
    runtime.writeLog('pre-skipped-no-candidate-files')
    runtime.continueSilently()
    return
  }

  runtime.removeState(runtime.recordPath(identity))
  runtime.writeState(runtime.snapshotPath(identity), {
    files: files.map(file => ({
      fingerprint: fingerprint(file),
      path: relative(runtime.cwd, file),
    })),
  })
  runtime.writeLog('pre-snapshot-created', {
    files: files.map(file => relative(runtime.cwd, file)),
  })
  runtime.continueSilently()
})
