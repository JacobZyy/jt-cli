import { cn } from "@workspace/ui/lib/utils"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"

export function MarkdownContent({ markdown, className }: { markdown: string, className?: string }) {
  return (
    <div className={cn("min-w-0 text-sm leading-6 text-foreground", className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ className: linkClassName, ...props }) => (
            <a className={cn("font-medium text-signal-blue underline underline-offset-4", linkClassName)} {...props} />
          ),
          blockquote: ({ className: quoteClassName, ...props }) => (
            <blockquote className={cn("my-3 border-l-2 border-signal-blue/35 pl-4 text-muted-foreground", quoteClassName)} {...props} />
          ),
          code: ({ className: codeClassName, ...props }) => (
            <code className={cn("rounded bg-signal-ink/[0.07] px-1.5 py-0.5 font-data text-[0.82em]", codeClassName)} {...props} />
          ),
          h1: props => <h1 className="mt-6 mb-3 text-xl font-semibold" {...props} />,
          h2: props => <h2 className="mt-5 mb-2 text-lg font-semibold" {...props} />,
          h3: props => <h3 className="mt-4 mb-2 font-semibold" {...props} />,
          li: props => <li className="ml-5 list-disc pl-1 marker:text-muted-foreground" {...props} />,
          ol: props => <ol className="my-3 space-y-1" {...props} />,
          p: props => <p className="my-2 first:mt-0 last:mb-0" {...props} />,
          pre: ({ className: preClassName, ...props }) => (
            <pre className={cn("my-3 overflow-x-auto rounded-lg bg-signal-ink p-4 font-data text-xs leading-5 text-[#e7eefb]", preClassName)} {...props} />
          ),
          table: props => <table className="my-4 w-full border-collapse overflow-hidden text-left text-xs" {...props} />,
          td: props => <td className="border border-border px-3 py-2 align-top" {...props} />,
          th: props => <th className="border border-border bg-muted px-3 py-2 font-semibold" {...props} />,
          ul: props => <ul className="my-3 space-y-1" {...props} />,
        }}
      >
        {markdown}
      </ReactMarkdown>
    </div>
  )
}
