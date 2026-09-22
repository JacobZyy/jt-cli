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
  - title: Gateway RPC contracts
    details: 按 RPC 方法首参是否来自 com.zhuanzhuan.arch.zgateway.support 包识别入口，不依赖 Facade 后缀；未配置网关的接口保留 Pending。
  - title: One Contract IR
    details: 同一份契约中间表示并行生成 OpenAPI、API、types 与 enums，避免多段 codegen 反复猜测语义。
  - title: Conservative semantics
    details: 只有证据闭合的值域才生成严格枚举；RPC、Database、歧义与截断结果保持开放。
---

## 从这里开始

- 想理解设计：阅读 [Why nlab-api](/why)。
- 想立即使用：阅读 [Quick Start](/quick-start)。
- 想了解后续计划：阅读 [Roadmap](/roadmap)。
