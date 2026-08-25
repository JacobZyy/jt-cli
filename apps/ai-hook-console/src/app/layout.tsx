import type { Metadata } from "next"
import type { ReactNode } from "react"

import "@workspace/ui/globals.css"

import { AppShell } from "@/components/app-shell"

export const metadata: Metadata = {
  description: "本地查看 Codex AI-hook 执行、会话与代码 patch。",
  title: "AI-hook Trace Desk",
}

export default function RootLayout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="zh-CN">
      <body>
        <AppShell>{children}</AppShell>
      </body>
    </html>
  )
}
