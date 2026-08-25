import { describe, expect, it } from "vitest"

import { createAiHookSession, parseHookLog, parseSessionDocument } from "./parse"
import type { HookLogEntry } from "./types"

function hookEntry(overrides: Partial<HookLogEntry>): HookLogEntry {
  return {
    durationMs: 12,
    files: [],
    hookEventName: "Stop",
    line: 1,
    message: null,
    runner: null,
    sessionId: "session-1",
    sourcePath: "/tmp/jt-ai-hook-demo.jsonl",
    status: "skipped-no-ai-edited-files",
    stopHookActive: false,
    timestamp: "2026-08-25T01:00:00.000Z",
    toolUseId: null,
    turnId: "turn-1",
    ...overrides,
  }
}

describe("AI-hook parsers", () => {
  it("keeps valid log evidence and reports malformed lines", () => {
    const result = parseHookLog([
      JSON.stringify({
        durationMs: 18,
        files: ["src/page.tsx"],
        hookEventName: "PostToolUse",
        sessionId: "session-1",
        status: "post-recorded-edits",
        timestamp: "2026-08-25T01:00:00.000Z",
      }),
      "not-json",
    ].join("\n"), "/tmp/jt-ai-hook-demo.jsonl")

    expect(result.entries).toHaveLength(1)
    expect(result.entries[0]).toMatchObject({
      files: ["src/page.tsx"],
      sessionId: "session-1",
      status: "post-recorded-edits",
    })
    expect(result.warnings).toEqual([
      expect.objectContaining({ line: 2, sourcePath: "/tmp/jt-ai-hook-demo.jsonl" }),
    ])
  })

  it("extracts useful user context and apply_patch diffs", () => {
    const content = [
      JSON.stringify({
        timestamp: "2026-08-25T01:00:00.000Z",
        type: "session_meta",
        payload: {
          cwd: "/workspace/demo",
          id: "session-1",
          timestamp: "2026-08-25T01:00:00.000Z",
        },
      }),
      JSON.stringify({
        timestamp: "2026-08-25T01:00:01.000Z",
        type: "response_item",
        payload: {
          type: "message",
          role: "user",
          content: [{
            type: "input_text",
            text: "<environment_context>noise</environment_context>\n实现 AI-hook 控制台",
          }],
        },
      }),
      JSON.stringify({
        timestamp: "2026-08-25T01:00:02.000Z",
        type: "response_item",
        payload: {
          type: "custom_tool_call",
          name: "exec",
          input: "const patch = \"*** Begin Patch\\n*** Update File: src/page.tsx\\n@@\\n-old\\n+new\\n*** End Patch\"",
          internal_chat_message_metadata_passthrough: { turn_id: "turn-1" },
        },
      }),
    ].join("\n")

    const result = parseSessionDocument([{ content, sourcePath: "/sessions/demo.jsonl" }], "fallback")

    expect(result.document).toMatchObject({
      context: "实现 AI-hook 控制台",
      cwd: "/workspace/demo",
      id: "session-1",
      title: "实现 AI-hook 控制台",
    })
    expect(result.document.patches[0]).toMatchObject({
      files: ["src/page.tsx"],
      turnId: "turn-1",
    })
    expect(result.document.patches[0]?.patch).toContain("+new")
  })

  it("uses the final Stop retry instead of an earlier failure", () => {
    const entries = [
      hookEntry({ runner: "eslint", status: "runner-passed" }),
      hookEntry({ runner: "vitest", status: "runner-failed" }),
      hookEntry({
        runner: "eslint",
        status: "runner-passed",
        stopHookActive: true,
        timestamp: "2026-08-25T01:03:00.000Z",
      }),
      hookEntry({
        runner: "vitest",
        status: "runner-passed",
        stopHookActive: true,
        timestamp: "2026-08-25T01:03:00.001Z",
      }),
    ]
    const session = createAiHookSession(entries, {
      context: "",
      cwd: "/workspace/demo",
      id: "session-1",
      messages: [],
      patches: [],
      sourcePaths: [],
      startedAt: null,
      title: "Demo",
    })

    expect(session.status).toBe("passed")
    expect(session.checkCount).toBe(2)
  })
})
