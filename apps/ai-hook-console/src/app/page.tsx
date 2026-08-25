import { loadAiHookData, type AiHookSession, type AiHookStatus } from "@workspace/ai-hook-core"
import { buttonVariants, Button } from "@workspace/ui/components/button"
import { Card, CardContent } from "@workspace/ui/components/card"
import { Input } from "@workspace/ui/components/input"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@workspace/ui/components/table"
import { cn } from "@workspace/ui/lib/utils"
import { ArrowUpRight, Filter, RotateCcw, Search } from "lucide-react"
import Link from "next/link"

import { DataWarnings } from "@/components/data-warnings"
import { StatusBadge } from "@/components/status-badge"
import { formatDateTime, shortPath, statusMeta } from "@/lib/format"

export const dynamic = "force-dynamic"

type SearchParams = Promise<Record<string, string | string[] | undefined>>

function valueOf(value: string | string[] | undefined): string {
  return Array.isArray(value) ? value[0] ?? "" : value ?? ""
}

function dateKey(value: string | null): string {
  if (!value) return ""
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ""
  return new Intl.DateTimeFormat("en-CA", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  }).format(date)
}

function matchesQuery(session: AiHookSession, query: string): boolean {
  if (!query) return true
  const haystack = [
    session.title,
    session.id,
    session.cwd ?? "",
    session.context,
    ...session.entries.flatMap(entry => entry.files),
  ].join("\n").toLocaleLowerCase()
  return haystack.includes(query.toLocaleLowerCase())
}

function contextSummary(markdown: string): string {
  const summary = markdown
    .replace(/\[([^\]]+)]\([^)]+\)/g, "$1")
    .replace(/[`*_#~]/g, "")
    .replace(/\s+/g, " ")
    .trim()
  return summary.length > 220 ? `${summary.slice(0, 219)}…` : summary
}

export default async function SessionsPage({ searchParams }: { searchParams: SearchParams }) {
  const [data, params] = await Promise.all([loadAiHookData(), searchParams])
  const query = valueOf(params.q).trim()
  const selectedStatus = valueOf(params.status)
  const selectedDate = valueOf(params.date)
  const selectedPath = valueOf(params.path)
  const sessions = data.sessions.filter(session => (
    matchesQuery(session, query)
    && (!selectedStatus || session.status === selectedStatus)
    && (!selectedDate || dateKey(session.lastExecutedAt) === selectedDate)
    && (!selectedPath || session.cwd === selectedPath)
  ))
  const pathOptions = [...new Set(data.sessions.flatMap(session => session.cwd ? [session.cwd] : []))]
    .toSorted()
  const totalTriggers = data.sessions.reduce((sum, session) => sum + session.triggerCount, 0)
  const failedSessions = data.sessions.filter(session => session.status === "failed").length
  const passedSessions = data.sessions.filter(session => session.status === "passed").length

  return (
    <div className="space-y-6">
      <header className="grid gap-5 lg:grid-cols-[1fr_auto] lg:items-end">
        <div>
          <p className="font-data text-xs font-semibold tracking-[0.2em] text-signal-blue">LOCAL OBSERVABILITY</p>
          <h1 className="mt-2 text-3xl font-semibold tracking-[-0.035em] text-signal-ink sm:text-4xl">
            AI-hook 会话记录
          </h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
            将执行结果、上下文、代码路径与 apply_patch 对比放进同一条会话时间线。
          </p>
        </div>
        <div className="rounded-lg border border-signal-grid bg-white/80 px-3 py-2 font-data text-xs text-muted-foreground shadow-sm backdrop-blur">
          refreshed {formatDateTime(data.loadedAt)}
        </div>
      </header>

      <DataWarnings warnings={data.warnings} />

      <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" aria-label="汇总">
        <Metric label="会话" value={data.sessions.length} hint="有 AI-hook 日志" tone="blue" />
        <Metric label="触发记录" value={totalTriggers} hint="Pre · Post · Stop" tone="ink" />
        <Metric label="当前通过" value={passedSessions} hint="最后一轮检查" tone="teal" />
        <Metric label="当前失败" value={failedSessions} hint="等待修复" tone="red" />
      </section>

      <Card className="border-0 bg-white/90 py-0 shadow-[0_18px_50px_rgba(41,60,93,0.09)] ring-1 ring-signal-grid/80 backdrop-blur">
        <CardContent className="p-4 sm:p-5">
          <form className="grid gap-3 lg:grid-cols-[minmax(15rem,1.4fr)_minmax(10rem,0.7fr)_minmax(10rem,1fr)_10rem_auto]" action="/">
            <label className="relative">
              <span className="sr-only">搜索</span>
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
              <Input name="q" defaultValue={query} placeholder="标题、会话 ID、代码路径" className="h-9 bg-white pl-9" />
            </label>
            <label>
              <span className="sr-only">状态</span>
              <select name="status" defaultValue={selectedStatus} className="h-9 w-full rounded-lg border border-input bg-white px-3 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50">
                <option value="">全部状态</option>
                {(Object.entries(statusMeta) as Array<[AiHookStatus, (typeof statusMeta)[AiHookStatus]]>)
                  .map(([status, meta]) => <option key={status} value={status}>{meta.label}</option>)}
              </select>
            </label>
            <label>
              <span className="sr-only">代码路径</span>
              <select name="path" defaultValue={selectedPath} className="h-9 w-full rounded-lg border border-input bg-white px-3 font-data text-xs outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50">
                <option value="">全部代码路径</option>
                {pathOptions.map(path => <option key={path} value={path}>{shortPath(path)}</option>)}
              </select>
            </label>
            <label>
              <span className="sr-only">日期</span>
              <Input name="date" type="date" defaultValue={selectedDate} className="h-9 bg-white" />
            </label>
            <div className="flex gap-2">
              <Button type="submit" size="lg" className="h-9 flex-1 bg-signal-ink hover:bg-signal-ink/85 lg:flex-none">
                <Filter data-icon="inline-start" aria-hidden="true" />
                筛选
              </Button>
              <Link href="/" className={cn(buttonVariants({ variant: "outline", size: "icon-lg" }), "h-9 bg-white")} aria-label="重置筛选">
                <RotateCcw aria-hidden="true" />
              </Link>
            </div>
          </form>
        </CardContent>
      </Card>

      <section className="overflow-hidden rounded-xl border border-signal-grid/90 bg-white/95 shadow-[0_18px_55px_rgba(41,60,93,0.08)]">
        <div className="flex items-center justify-between border-b border-signal-grid/75 px-4 py-3 sm:px-5">
          <div>
            <h2 className="text-sm font-semibold text-signal-ink">会话列表</h2>
            <p className="mt-0.5 text-xs text-muted-foreground">{sessions.length} / {data.sessions.length} 条</p>
          </div>
          <span className="font-data text-[0.68rem] tracking-[0.14em] text-muted-foreground">LATEST FIRST</span>
        </div>
        {sessions.length > 0 ? (
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="w-2 px-0"><span className="sr-only">状态信号</span></TableHead>
                <TableHead className="min-w-72">会话</TableHead>
                <TableHead>状态</TableHead>
                <TableHead className="text-right">触发</TableHead>
                <TableHead className="text-right">检查</TableHead>
                <TableHead className="min-w-64">代码路径</TableHead>
                <TableHead className="min-w-44">最后执行</TableHead>
                <TableHead className="w-12"><span className="sr-only">查看</span></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {sessions.map(session => (
                <TableRow key={session.id} className="group hover:bg-[#f7f9fd]">
                  <TableCell className="relative p-0">
                    <span className={cn("absolute inset-y-0 left-0 w-1", signalClass(session.status))} aria-hidden="true" />
                  </TableCell>
                  <TableCell className="max-w-xl whitespace-normal py-4">
                    <Link prefetch={false} href={`/sessions/${encodeURIComponent(session.id)}`} className="font-semibold text-signal-ink underline-offset-4 hover:text-signal-blue hover:underline">
                      {session.title}
                    </Link>
                    <p className="mt-1 line-clamp-2 max-w-2xl text-xs leading-5 text-muted-foreground">
                      {session.context ? contextSummary(session.context) : "未找到会话上下文"}
                    </p>
                    <p className="mt-2 font-data text-[0.68rem] text-muted-foreground">{session.id}</p>
                  </TableCell>
                  <TableCell><StatusBadge status={session.status} /></TableCell>
                  <TableCell className="text-right font-data tabular-nums">{session.triggerCount}</TableCell>
                  <TableCell className="text-right font-data tabular-nums">{session.checkCount}</TableCell>
                  <TableCell className="max-w-sm whitespace-normal font-data text-xs leading-5 text-muted-foreground">{shortPath(session.cwd)}</TableCell>
                  <TableCell className="font-data text-xs text-muted-foreground">{formatDateTime(session.lastExecutedAt)}</TableCell>
                  <TableCell>
                    <Link prefetch={false} href={`/sessions/${encodeURIComponent(session.id)}`} className="grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-signal-blue/10 hover:text-signal-blue" aria-label={`查看 ${session.title}`}>
                      <ArrowUpRight className="size-4" aria-hidden="true" />
                    </Link>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <div className="px-6 py-16 text-center">
            <Search className="mx-auto size-6 text-muted-foreground" aria-hidden="true" />
            <h3 className="mt-3 font-semibold">没有匹配会话</h3>
            <p className="mt-1 text-sm text-muted-foreground">调整筛选条件后重试。</p>
          </div>
        )}
      </section>
    </div>
  )
}

function Metric({ label, value, hint, tone }: { label: string, value: number, hint: string, tone: "blue" | "ink" | "red" | "teal" }) {
  const toneClass = {
    blue: "text-signal-blue",
    ink: "text-signal-ink",
    red: "text-signal-red",
    teal: "text-signal-teal",
  }[tone]
  return (
    <Card className="gap-0 border-0 bg-white/90 py-4 shadow-sm ring-1 ring-signal-grid/80 backdrop-blur">
      <CardContent className="flex items-end justify-between px-4">
        <div>
          <p className="text-xs font-medium text-muted-foreground">{label}</p>
          <p className={cn("mt-1 font-data text-3xl font-semibold tracking-[-0.06em]", toneClass)}>{value}</p>
        </div>
        <p className="pb-1 text-[0.68rem] text-muted-foreground">{hint}</p>
      </CardContent>
    </Card>
  )
}

function signalClass(status: AiHookStatus): string {
  return {
    failed: "bg-signal-red",
    passed: "bg-signal-teal",
    pending: "bg-signal-blue",
    skipped: "bg-signal-grid",
    warning: "bg-signal-amber",
  }[status]
}
