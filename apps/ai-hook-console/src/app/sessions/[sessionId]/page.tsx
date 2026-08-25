import {
  loadAiHookSession,
  type AiHookStatus,
  type HookLogEntry,
} from "@workspace/ai-hook-core"
import { Badge } from "@workspace/ui/components/badge"
import { buttonVariants } from "@workspace/ui/components/button"
import { Card, CardContent, CardHeader, CardTitle } from "@workspace/ui/components/card"
import { cn } from "@workspace/ui/lib/utils"
import {
  ArrowLeft,
  Braces,
  Clock3,
  FileCode2,
  FolderGit2,
  MessageSquareText,
  Radio,
  Route,
  TimerReset,
} from "lucide-react"
import Link from "next/link"
import { notFound } from "next/navigation"

import { DataWarnings } from "@/components/data-warnings"
import { DiffViewer } from "@/components/diff-viewer"
import { MarkdownContent } from "@/components/markdown-content"
import { StatusBadge } from "@/components/status-badge"
import { formatDateTime, formatDuration, shortPath } from "@/lib/format"

export const dynamic = "force-dynamic"

export default async function SessionDetailPage({ params }: { params: Promise<{ sessionId: string }> }) {
  const { sessionId } = await params
  const data = await loadAiHookSession(sessionId)
  if (!data.session) notFound()
  const session = data.session

  return (
    <div className="space-y-6">
      <Link href="/" className={cn(buttonVariants({ variant: "outline", size: "sm" }), "bg-white/90")}>
        <ArrowLeft data-icon="inline-start" aria-hidden="true" />
        返回会话列表
      </Link>

      <DataWarnings warnings={data.warnings} />

      <header className="relative overflow-hidden rounded-2xl bg-signal-ink p-6 text-white shadow-[0_28px_70px_rgba(22,36,58,0.24)] sm:p-8">
        <div className="absolute inset-y-0 left-0 w-1.5 bg-signal-blue" aria-hidden="true" />
        <div className="absolute top-0 right-0 h-full w-1/3 bg-[radial-gradient(circle_at_top_right,rgba(47,91,255,0.32),transparent_64%)]" aria-hidden="true" />
        <div className="relative">
          <div className="flex flex-wrap items-center gap-3">
            <StatusBadge status={session.status} className="border-white/15 bg-white/10 text-white" />
            <span className="font-data text-xs text-white/50">{session.id}</span>
          </div>
          <h1 className="mt-5 max-w-5xl text-2xl font-semibold tracking-[-0.03em] sm:text-4xl">{session.title}</h1>
          {session.context ? (
            <MarkdownContent markdown={session.context} className="mt-3 max-w-4xl text-sm leading-6 text-white/65 [&_a]:text-[#aabaff]" />
          ) : <p className="mt-3 text-sm text-white/65">未找到会话上下文。</p>}
          <div className="mt-6 flex flex-wrap gap-x-6 gap-y-3 border-t border-white/10 pt-5 text-xs text-white/55">
            <span className="flex items-center gap-2"><FolderGit2 className="size-4" aria-hidden="true" />{shortPath(session.cwd)}</span>
            <span className="flex items-center gap-2"><Clock3 className="size-4" aria-hidden="true" />{formatDateTime(session.lastExecutedAt)}</span>
            <span className="flex items-center gap-2"><FileCode2 className="size-4" aria-hidden="true" />{session.fileCount} 个代码文件</span>
          </div>
        </div>
      </header>

      <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" aria-label="会话汇总">
        <DetailMetric icon={Radio} label="触发记录" value={session.triggerCount} />
        <DetailMetric icon={TimerReset} label="Stop 检查" value={session.checkCount} />
        <DetailMetric icon={Braces} label="记录编辑" value={session.editCount} />
        <DetailMetric icon={Route} label="代码 Patch" value={session.patches.length} />
      </section>

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1.35fr)_minmax(21rem,0.65fr)] xl:items-start">
        <section className="space-y-4">
          <SectionHeading eyebrow="HOOK EVENTS" title="执行时间线" count={session.entries.length} />
          <div className="rounded-xl border border-signal-grid/90 bg-white/95 p-4 shadow-[0_16px_45px_rgba(41,60,93,0.07)] sm:p-5">
            <div className="space-y-0">
              {session.entries.toReversed().map(entry => (
                <HookEvent key={`${entry.sourcePath}:${entry.line}`} entry={entry} />
              ))}
            </div>
          </div>
        </section>

        <aside className="space-y-5 xl:sticky xl:top-6">
          <Card className="border-0 bg-white/95 shadow-[0_16px_45px_rgba(41,60,93,0.07)] ring-1 ring-signal-grid/90">
            <CardHeader className="border-b border-signal-grid/70">
              <CardTitle className="text-sm">会话信息</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4 text-xs">
              <Info label="开始时间" value={formatDateTime(session.startedAt)} />
              <Info label="工作目录" value={shortPath(session.cwd)} mono />
              <Info label="日志文件" value={session.logPaths.map(shortPath).join("\n") || "—"} mono />
              <Info label="会话文件" value={session.sourcePaths.map(shortPath).join("\n") || "未找到"} mono />
            </CardContent>
          </Card>

          <Card className="border-0 bg-white/95 shadow-[0_16px_45px_rgba(41,60,93,0.07)] ring-1 ring-signal-grid/90">
            <CardHeader className="border-b border-signal-grid/70">
              <CardTitle className="flex items-center gap-2 text-sm">
                <MessageSquareText className="size-4 text-signal-blue" aria-hidden="true" />
                会话内容 · {session.messages.length}
              </CardTitle>
            </CardHeader>
            <CardContent className="max-h-[42rem] space-y-3 overflow-auto">
              {session.messages.length > 0 ? session.messages.map(message => (
                <details key={`${message.sourcePath}:${message.line}`} className="rounded-lg border border-border bg-background/60 px-3 py-2 open:bg-white">
                  <summary className="cursor-pointer list-none text-xs font-medium">
                    <span className={message.role === "user" ? "text-signal-blue" : "text-signal-teal"}>
                      {message.role === "user" ? "用户" : "Codex"}
                    </span>
                    <span className="ml-2 font-data text-[0.65rem] text-muted-foreground">{formatDateTime(message.timestamp)}</span>
                  </summary>
                  <MarkdownContent markdown={message.markdown} className="mt-3 border-t border-border pt-3" />
                </details>
              )) : <p className="text-sm text-muted-foreground">未找到会话消息。</p>}
            </CardContent>
          </Card>
        </aside>
      </div>

      <section className="space-y-4">
        <SectionHeading eyebrow="APPLY PATCH" title="代码对比" count={session.patches.length} />
        {session.patches.length > 0 ? session.patches.toReversed().map((patch, index) => (
          <Card key={`${patch.sourcePath}:${patch.line}`} className="gap-0 border-0 bg-white/95 py-0 shadow-[0_16px_45px_rgba(41,60,93,0.07)] ring-1 ring-signal-grid/90">
            <CardHeader className="border-b border-signal-grid/70 py-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <CardTitle className="text-sm">Patch {session.patches.length - index}</CardTitle>
                  <p className="mt-1 font-data text-[0.68rem] text-muted-foreground">{formatDateTime(patch.timestamp)} · {patch.turnId ?? "unknown turn"}</p>
                </div>
                <Badge variant="outline" className="rounded-md font-data text-[0.68rem]">{patch.files.length} files</Badge>
              </div>
              {patch.files.length > 0 ? (
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {patch.files.map(file => <code key={file} className="rounded bg-muted px-2 py-1 font-data text-[0.68rem] text-muted-foreground">{file}</code>)}
                </div>
              ) : null}
            </CardHeader>
            <CardContent className="p-3 sm:p-4"><DiffViewer patch={patch.patch} /></CardContent>
          </Card>
        )) : (
          <Card className="border-dashed bg-white/75 py-10 text-center">
            <CardContent>
              <Braces className="mx-auto size-6 text-muted-foreground" aria-hidden="true" />
              <p className="mt-3 text-sm font-medium">没有 apply_patch 记录</p>
              <p className="mt-1 text-xs text-muted-foreground">只读会话或日志轮转后会出现此状态。</p>
            </CardContent>
          </Card>
        )}
      </section>
    </div>
  )
}

function HookEvent({ entry }: { entry: HookLogEntry }) {
  const status = entryStatus(entry)
  const output = typeof entry.details?.output === "string" ? entry.details.output : null
  const extraDetails = entry.details ? Object.fromEntries(
    Object.entries(entry.details).filter(([key]) => key !== "output"),
  ) : null
  return (
    <article className="relative grid grid-cols-[1.25rem_minmax(0,1fr)] gap-3 pb-5 last:pb-0">
      <div className="relative flex justify-center">
        <span className="absolute top-3 bottom-[-1.25rem] w-px bg-signal-grid last:hidden" aria-hidden="true" />
        <span className={cn("relative z-10 mt-1 size-2.5 rounded-full ring-4 ring-white", eventDot(status))} aria-hidden="true" />
      </div>
      <div className="min-w-0 rounded-lg border border-signal-grid/80 bg-[#fbfcff] px-3 py-3 sm:px-4">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <StatusBadge status={status} />
            <span className="text-xs font-semibold text-signal-ink">{entry.hookEventName ?? "Unknown event"}</span>
            {entry.runner ? <Badge variant="outline" className="rounded-md font-data text-[0.65rem]">{entry.runner}</Badge> : null}
            <code className="font-data text-[0.68rem] text-muted-foreground">{entry.status}</code>
          </div>
          <div className="text-right font-data text-[0.65rem] leading-5 text-muted-foreground">
            <div>{formatDateTime(entry.timestamp)}</div>
            <div>{formatDuration(entry.durationMs)}</div>
          </div>
        </div>

        {entry.files.length > 0 ? (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {entry.files.map(file => <code key={file} className="rounded bg-white px-2 py-1 font-data text-[0.65rem] text-muted-foreground ring-1 ring-border">{file}</code>)}
          </div>
        ) : null}
        {entry.message ? <MarkdownContent markdown={entry.message} className="mt-3 border-t border-signal-grid/70 pt-3" /> : null}
        {output ? <pre className="mt-3 max-h-80 overflow-auto rounded-lg bg-signal-ink p-3 font-data text-[0.68rem] leading-5 text-[#dce6f5]">{output}</pre> : null}
        {extraDetails && Object.keys(extraDetails).length > 0 ? (
          <details className="mt-3 text-xs">
            <summary className="cursor-pointer text-muted-foreground">结构化详情</summary>
            <pre className="mt-2 max-h-72 overflow-auto rounded-lg bg-muted p-3 font-data text-[0.68rem] leading-5">{JSON.stringify(extraDetails, null, 2)}</pre>
          </details>
        ) : null}
        <p className="mt-3 font-data text-[0.62rem] text-muted-foreground/75">{shortPath(entry.sourcePath)}:{entry.line}</p>
      </div>
    </article>
  )
}

function entryStatus(entry: HookLogEntry): AiHookStatus {
  if (entry.status === "runner-failed" || entry.status === "hook-runtime-error") return "failed"
  if (entry.status.includes("warning")) return "warning"
  if (entry.status === "runner-passed" || entry.status === "post-recorded-edits") return "passed"
  if (entry.status.startsWith("skipped-") || entry.status.startsWith("pre-skipped-") || entry.status.startsWith("post-skipped-")) return "skipped"
  return "pending"
}

function eventDot(status: AiHookStatus): string {
  return {
    failed: "bg-signal-red",
    passed: "bg-signal-teal",
    pending: "bg-signal-blue",
    skipped: "bg-signal-grid",
    warning: "bg-signal-amber",
  }[status]
}

function SectionHeading({ eyebrow, title, count }: { eyebrow: string, title: string, count: number }) {
  return (
    <div className="flex items-end justify-between gap-3">
      <div>
        <p className="font-data text-[0.66rem] font-semibold tracking-[0.18em] text-signal-blue">{eyebrow}</p>
        <h2 className="mt-1 text-xl font-semibold tracking-[-0.02em] text-signal-ink">{title}</h2>
      </div>
      <span className="font-data text-xs text-muted-foreground">{count} records</span>
    </div>
  )
}

function DetailMetric({ icon: Icon, label, value }: { icon: typeof Radio, label: string, value: number }) {
  return (
    <Card className="gap-0 border-0 bg-white/92 py-4 shadow-sm ring-1 ring-signal-grid/80">
      <CardContent className="flex items-center gap-3 px-4">
        <span className="grid size-9 place-items-center rounded-lg bg-signal-blue/8 text-signal-blue"><Icon className="size-4" aria-hidden="true" /></span>
        <div><p className="text-xs text-muted-foreground">{label}</p><p className="font-data text-xl font-semibold text-signal-ink">{value}</p></div>
      </CardContent>
    </Card>
  )
}

function Info({ label, value, mono = false }: { label: string, value: string, mono?: boolean }) {
  return (
    <div>
      <p className="text-[0.68rem] font-medium text-muted-foreground">{label}</p>
      <p className={cn("mt-1 whitespace-pre-wrap break-all leading-5 text-foreground", mono && "font-data text-[0.68rem]")}>{value}</p>
    </div>
  )
}
