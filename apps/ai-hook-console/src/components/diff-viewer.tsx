import { cn } from "@workspace/ui/lib/utils"

function lineTone(line: string): string {
  if (line.startsWith("+") && !line.startsWith("+++")) return "bg-emerald-400/10 text-emerald-200"
  if (line.startsWith("-") && !line.startsWith("---")) return "bg-rose-400/10 text-rose-200"
  if (line.startsWith("@@")) return "bg-sky-400/10 text-sky-200"
  if (line.startsWith("***")) return "text-[#a9b8d0]"
  return "text-[#dce6f5]"
}

export function DiffViewer({ patch }: { patch: string }) {
  return (
    <div className="overflow-x-auto rounded-xl border border-white/10 bg-signal-ink py-3 shadow-[0_18px_40px_rgba(22,36,58,0.16)]">
      <pre className="min-w-max font-data text-xs leading-5">
        {patch.split("\n").map((line, index) => (
          <span key={`${index}-${line.slice(0, 24)}`} className={cn("block min-h-5 px-4", lineTone(line))}>
            <span className="mr-4 inline-block w-8 select-none text-right text-white/25">{index + 1}</span>
            {line || " "}
          </span>
        ))}
      </pre>
    </div>
  )
}
