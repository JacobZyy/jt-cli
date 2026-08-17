// jt-ai-hook
// Records files whose content changed after an AI patch.

import { relative } from 'node:path'

import { fingerprint, safeFilePath } from './files'
import { isRecord } from './protocol'
import { runStage } from './runtime'

runStage('PostToolUse', (runtime) => {
  const { input } = runtime
  if (input.tool_name !== 'apply_patch') {
    runtime.writeLog('post-skipped-unsupported-tool', { toolName: input.tool_name || null })
    runtime.continueSilently()
    return
  }

  const identity = runtime.stateIdentity(true)
  if (!identity) {
    runtime.writeLog('post-skipped-missing-identity')
    runtime.continueSilently()
    return
  }

  const snapshotPath = runtime.snapshotPath(identity)
  const snapshot = runtime.readState(snapshotPath)
  runtime.removeState(snapshotPath)
  if (!isRecord(snapshot) || !Array.isArray(snapshot.files)) {
    runtime.writeLog('post-skipped-snapshot-missing')
    runtime.continueSilently()
    return
  }

  const changedFiles: string[] = []
  for (const rawEntry of snapshot.files) {
    if (!isRecord(rawEntry) || typeof rawEntry.path !== 'string' || !isRecord(rawEntry.fingerprint))
      continue
    const before = rawEntry.fingerprint
    if (typeof before.exists !== 'boolean' || (typeof before.hash !== 'string' && before.hash !== null))
      continue
    const file = safeFilePath(runtime.cwd, rawEntry.path)
    if (!file)
      continue
    const after = fingerprint(file)
    if (before.exists !== after.exists || before.hash !== after.hash)
      changedFiles.push(relative(runtime.cwd, file))
  }

  const files = [...new Set(changedFiles)].sort()
  if (files.length > 0)
    runtime.writeState(runtime.recordPath(identity), { files })
  else
    runtime.removeState(runtime.recordPath(identity))

  runtime.writeLog(files.length > 0 ? 'post-recorded-edits' : 'post-no-content-change', { files })
  runtime.continueSilently()
})
