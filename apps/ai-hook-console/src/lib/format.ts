import type { AiHookStatus } from "@workspace/ai-hook-core"

export const statusMeta = {
  failed: { label: "失败", tone: "destructive" },
  passed: { label: "通过", tone: "success" },
  pending: { label: "执行中", tone: "info" },
  skipped: { label: "跳过", tone: "muted" },
  warning: { label: "警告", tone: "warning" },
} as const satisfies Record<AiHookStatus, { label: string, tone: string }>

export function formatDateTime(value: string | null): string {
  if (!value) return "—"
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(date)
}

export function formatDuration(value: number | null): string {
  if (value === null) return "—"
  if (value < 1_000) return `${value} ms`
  return `${(value / 1_000).toFixed(value < 10_000 ? 1 : 0)} s`
}

export function shortPath(path: string | null): string {
  if (!path) return "未解析路径"
  return path
    .replace(/^\/Users\/[^/]+/, "~")
    .replace(/^\/home\/[^/]+/, "~")
}
