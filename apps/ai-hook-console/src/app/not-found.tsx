import { buttonVariants } from "@workspace/ui/components/button"
import { Card, CardContent } from "@workspace/ui/components/card"
import { SearchX } from "lucide-react"
import Link from "next/link"

export default function NotFound() {
  return (
    <Card className="mx-auto mt-20 max-w-xl border-0 bg-white/95 py-10 text-center shadow-xl ring-1 ring-signal-grid">
      <CardContent>
        <SearchX className="mx-auto size-8 text-muted-foreground" aria-hidden="true" />
        <h1 className="mt-4 text-xl font-semibold">未找到会话</h1>
        <p className="mt-2 text-sm text-muted-foreground">日志可能已轮转，或会话 ID 不存在。</p>
        <Link href="/" className={buttonVariants({ className: "mt-6", variant: "default" })}>返回列表</Link>
      </CardContent>
    </Card>
  )
}
