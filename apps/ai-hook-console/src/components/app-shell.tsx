import { Activity, CircleGauge, FileCode2, RadioTower } from "lucide-react"
import Link from "next/link"
import type { ReactNode } from "react"

export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-svh md:grid md:grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="border-b border-signal-grid/90 bg-signal-ink px-5 py-5 text-white md:sticky md:top-0 md:h-svh md:border-r md:border-b-0">
        <Link href="/" className="flex items-center gap-3 focus-visible:rounded-md focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-white">
          <span className="grid size-9 place-items-center rounded-lg bg-signal-blue shadow-[0_0_0_4px_rgba(47,91,255,0.18)]">
            <RadioTower className="size-4" aria-hidden="true" />
          </span>
          <span>
            <span className="block text-sm font-semibold tracking-[0.14em]">TRACE DESK</span>
            <span className="block text-[0.68rem] text-white/55">AI-hook flight recorder</span>
          </span>
        </Link>

        <nav className="mt-5 flex gap-2 md:mt-10 md:block md:space-y-2" aria-label="主导航">
          <Link
            href="/"
            className="flex items-center gap-2 rounded-lg border border-white/12 bg-white/9 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-white/14"
          >
            <Activity className="size-4 text-[#8aa4ff]" aria-hidden="true" />
            会话记录
          </Link>
          <span className="hidden items-center gap-2 rounded-lg px-3 py-2 text-sm text-white/45 md:flex">
            <CircleGauge className="size-4" aria-hidden="true" />
            本地只读
          </span>
        </nav>

        <div className="mt-auto hidden border-t border-white/10 pt-4 text-xs leading-5 text-white/45 md:absolute md:right-5 md:bottom-5 md:left-5 md:block">
          <FileCode2 className="mb-2 size-4" aria-hidden="true" />
          数据直接读取 /tmp 与 ~/.codex。
          <br />
          不上传，不改写日志。
        </div>
      </aside>
      <main className="min-w-0 px-4 py-6 sm:px-7 lg:px-10 lg:py-9">
        <div className="mx-auto w-full max-w-[92rem]">{children}</div>
      </main>
    </div>
  )
}
