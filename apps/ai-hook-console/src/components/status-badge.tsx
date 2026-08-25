import type { AiHookStatus } from "@workspace/ai-hook-core"
import { Badge } from "@workspace/ui/components/badge"
import { cn } from "@workspace/ui/lib/utils"

import { statusMeta } from "@/lib/format"

const toneClass = {
  destructive: "border-signal-red/20 bg-signal-red/10 text-signal-red",
  info: "border-signal-blue/20 bg-signal-blue/10 text-signal-blue",
  muted: "border-border bg-muted text-muted-foreground",
  success: "border-signal-teal/20 bg-signal-teal/10 text-signal-teal",
  warning: "border-signal-amber/20 bg-signal-amber/10 text-signal-amber",
} as const

export function StatusBadge({ status, className }: { status: AiHookStatus, className?: string }) {
  const meta = statusMeta[status]
  return (
    <Badge
      variant="outline"
      className={cn("gap-1.5 rounded-md px-2 py-0.5 font-data", toneClass[meta.tone], className)}
    >
      <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
      {meta.label}
    </Badge>
  )
}
