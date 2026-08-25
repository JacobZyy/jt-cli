import type { DataWarning } from "@workspace/ai-hook-core"
import { AlertTriangle } from "lucide-react"

export function DataWarnings({ warnings }: { warnings: DataWarning[] }) {
  if (warnings.length === 0) return null
  return (
    <details className="rounded-xl border border-signal-amber/25 bg-[#fff8e8] px-4 py-3 text-sm text-[#744900]">
      <summary className="flex cursor-pointer list-none items-center gap-2 font-medium">
        <AlertTriangle className="size-4" aria-hidden="true" />
        {warnings.length} 条日志读取警告
      </summary>
      <ul className="mt-3 max-h-56 space-y-2 overflow-auto border-t border-signal-amber/15 pt-3 font-data text-xs leading-5">
        {warnings.slice(0, 50).map((warning, index) => (
          <li key={`${warning.sourcePath}-${warning.line ?? index}`}>
            {warning.sourcePath}{warning.line ? `:${warning.line}` : ""} · {warning.message}
          </li>
        ))}
      </ul>
    </details>
  )
}
