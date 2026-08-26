import { defineConfig } from "vitepress"

export default defineConfig({
  lang: "zh-CN",
  title: "nlab-api",
  description: "从 NLab Java Facade 生成前端可用契约",
  cleanUrls: true,
  themeConfig: {
    nav: [
      { text: "Why nlab-api", link: "/why" },
      { text: "Quick Start", link: "/quick-start" },
      { text: "Roadmap", link: "/roadmap" },
    ],
    sidebar: [
      {
        text: "nlab-api",
        items: [
          { text: "设计思路", link: "/why" },
          { text: "快速开始", link: "/quick-start" },
          { text: "Roadmap", link: "/roadmap" },
        ],
      },
    ],
    outline: {
      level: [2, 3],
      label: "本页内容",
    },
    search: {
      provider: "local",
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/JacobZyy/jt-cli" },
    ],
    footer: {
      message: "Deterministic contracts. Conservative semantics.",
      copyright: "MIT Licensed",
    },
  },
})
