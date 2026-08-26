---
layout: home

hero:
  name: nlab-api
  text: 从 Java Facade 到前端可用契约
  tagline: 以真实服务契约为根，生成 OpenAPI、TypeScript 类型、API 客户端与可追溯枚举。
  actions:
    - theme: brand
      text: 快速开始
      link: /quick-start
    - theme: alt
      text: 为什么需要 nlab-api
      link: /why

features:
  - title: Facade-rooted
    details: 从对外 Facade 方法确定 operation、请求类型与响应类型，不把内部 Service 当成公共契约。
  - title: One Contract IR
    details: 同一份契约中间表示并行生成 OpenAPI、API、types 与 enums，避免多段 codegen 反复猜测语义。
  - title: Conservative semantics
    details: 只有证据闭合的值域才生成严格枚举；RPC、Database、歧义与截断结果保持开放。
---

## 从这里开始

- 想理解设计：阅读 [Why nlab-api](/why)。
- 想立即使用：阅读 [Quick Start](/quick-start)。
- 想了解后续计划：阅读 [Roadmap](/roadmap)。
