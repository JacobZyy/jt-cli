# jt harness memo：Rust 归拢设计

状态：按 2026-09-16 用户最新指令修订。唯一产品仓库为 `/Users/jacobzha/Documents/workspace/jacob-open-source/jt-cli`，入口为已有 Rust 二进制 `jt` 的 `harness memo` 子命令。

## 当前目标

在现有 jt-cli 内用 Rust 完成记忆的提交、后台处理、DSH Agent 调用、Embedding、PostgreSQL/pgvector 写入、查询与状态查看。复用已验收的提炼规则和数据结构；DSH 继续承担模型与 Agent 执行。完成必要检查及真实 API 联调，不继续扩展 jt-harness 下的 TypeScript 存储插件，不创建独立应用或自研 TypeScript 服务。Codex Hook 自动安装与流程控制后置。

## 职责归属

| 内容 | 负责人 |
|---|---|
| `jt harness memo` 命令、配置、输入输出 | jt-cli / Rust |
| 收件、持久队列、重试与状态 | jt-cli / Rust，复用已有 rusqlite |
| 启动 DSH、指定模型、传递会话、收集结果 | jt-cli / Rust 的 DSH 协议适配 |
| 记忆提炼 | 已验收 Agent，由外部 DSH 运行 |
| 输出校验、Embedding HTTP 调用 | jt-cli / Rust |
| PostgreSQL/pgvector、事务、幂等、来源和范围 | jt-cli / Rust |
| 查询、详情、提交回执 | jt-cli / Rust |

“纯 Rust”指自研的前置与后置逻辑。DSH 是外部运行时，继续使用用户安装的 `dsh`，不重写 DSH。Agent 的 Markdown 规则、JSON Schema 和 DSH profile patch 是配置资源，随 jt-cli 管理。

## DSH 调用选择

Rust 直接启动 `dsh --profile sdk-minimal --patch <jt 管理的 Agent 配置>`，通过标准输入输出使用 DSH SDK 运行时的 JSON-RPC 协议。已核对本机 DSH 的公开请求为 `initialize`、`session/prompt`、`shutdown`；初始化支持独立指定 provider、model、reasoningEffort 和 maxTokens。

这承接之前验证过的 SDK 执行方式，但不再直接调用 TypeScript SDK 包。Rust 侧需要实现窄的协议适配，不声称 DSH 已提供官方 Rust SDK。

适配必须保留已验证的完成语义：

- `session/prompt` 返回的 messageId 只代表进入收件箱，不代表任务完成。
- 先观察到本次消息的收件确认，再等待目标会话结束；不能把任意一次 `idle` 当作完成。
- 只有 `turn/end` 为 completed、最终文本通过结构与来源校验，才能进入写入。
- 保留无工具提炼配置；异常工具调用应使本次处理失败。
- 超时、协议错误和退出都由 Rust 关闭并回收子进程，保留可诊断错误。

## 命令与后台执行

计划提供：

```text
jt harness memo init
jt harness memo send <file|->
jt harness memo status [submission-id]
jt harness memo search <query>
jt harness memo read <memory-id>
```

后台处理入口仍属于同一个 `jt` 二进制，可由 `send` 启动；需要恢复任务时也能显式执行。`send` 默认在原始材料可靠接收后返回，不等待 DSH 或 Embedding。用于人工联调的等待模式必须显式选择。

SQLite 只管理投递及处理中的任务，不成为第二套可搜索的记忆库。PostgreSQL 是唯一正式记忆库。任务提交成功与知识提交成功分别报告。

处理顺序：

1. 验证输入并保存原始批次，生成或复用任务回执。
2. 后台读取批次；已存在 PostgreSQL 提交回执时直接恢复完成状态。
3. 调用 DSH，保存通过校验的提炼结果，避免 Embedding 失败后重复提炼。
4. Rust 调用 Embedding，校验条数、索引、维度及有限数值。
5. PostgreSQL 事务写入原始来源、候选记录、向量及提交回执。
6. 更新本地任务状态。若在数据库提交后中断，下次以数据库回执恢复，不重复写入。

网络调用不放进数据库事务。重试保留同一批次身份；同 ID 不同内容明确报冲突。后台互斥与崩溃恢复由程序负责，不让模型模拟锁、队列或事务。

## 数据规则

- 输入继续采用现有 `schema_version / submission_id / source / scope / messages` 结构。
- 输出继续区分 `memories / proposals / revisions`，保留依据类别与原始消息引用。
- 未确认建议不能成为已确认事实；`current_task` 只在对应来源会话范围内读取。
- 每个向量空间明确绑定模型、维度和文本编码规则，不能只因维度相同就混用。
- 当前修订结果只有前后文本，没有既有数据库目标 ID。首版保存修订证据，不按文字相似度静默覆盖跨批旧知识。
- 查询先执行范围过滤，再进行向量排序；多项目归属尚未明确到单条时采用保守过滤。
- 空提炼结果也保留已处理回执。C15 的流水过滤已被用户接受，不增加新的语义测试门槛。

## 代码落点与复用

在 `apps/jt/src/main.rs` 注册 `Harness` 命令。实现落在 `apps/jt/src/harness/memo/`，按实际职责划分命令、任务处理、DSH 协议、Embedding 和数据库模块；首版不另建 Cargo crate 或应用。

Agent 规则与必要配置资源放入 `apps/jt/templates/harness/memo/`。复用现有 Clap、Serde、rusqlite、SHA-256 和临时目录能力；HTTP 与 PostgreSQL 使用成熟 Rust 客户端，版本需满足仓库 Rust 1.85 的要求。

`jt ai-hook` 已有 Hook 配置所有权逻辑，后续自动安装应在该边界复用，不创建第二个竞争修改 Codex 配置的安装器。当前先完成手工调用的记忆闭环。

## 迁移及验证顺序

1. 归拢 Agent 规则、输入输出约定和必要样本，保持已经验收的提炼行为。
2. 接入 Rust 命令和持久收件，确认 `send` 的返回语义。
3. 实现 Rust DSH 协议适配，验证指定模型、完整输出、失败和超时处理。
4. 接入已经配置并验证过的 Embedding API，再完成 PostgreSQL 写入和查回。
5. 验证重复提交、异常中断、事务回滚、范围过滤和恢复。
6. 通过仓库检查后集成回主要工作区；不推送、不发布版本。

原 jt-harness 目录中的设计和评测保留为历史证据。未完成的 TypeScript 存储包退出实现路线，不部署为服务、不与 Rust 实现并行维护。`.env.local` 凭据不复制进 Git；联调从用户已提供的文件或显式环境变量读取，不打印密钥。

本机尚未发现现成 PostgreSQL 实例。实现时需在明确的隔离测试库验证真实 PostgreSQL/pgvector 行为；不能将先前 PGlite 依赖或虚构向量当作正式链路完成依据。
