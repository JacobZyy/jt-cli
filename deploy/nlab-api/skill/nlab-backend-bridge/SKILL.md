---
name: nlab-backend-bridge
metadata:
  version: 2.6.2
  opencode/autoinvoke: "false"
  invocation: manual
description:
  仅供用户显式调用 `$nlab-backend-bridge` 或客户端对应的手动 Skill 命令。按项目本地 config 选择 `jt nlab-api` 或 standalone `nlab-api`，从后端接口目录和网关 HTTP 路由识别入口，生成契约、OpenAPI、TypeScript 类型和 API，处理跨仓库 Discover 与获取失败报告。Rust CLI 独占仓库操作、配置、契约与枚举解析及生成逻辑。本 Skill 收集输入、触发命令、反馈报告；用户要求 Mock 时整理规则与编排。不是通用 Git、接口调试、代理或网关查询工具。
allowed-tools:
  - Bash
  - Read
  - AskUserQuestion
  - request_user_input
---

# nlab-backend-bridge

调用 `nlab-api`，将 Java 后端契约同步到前端项目。本版本对应 CLI 2.2.3 及以上的入口识别规则。

## 边界

- 仅在用户显式调用本 Skill 时执行。
- runner config 是命令选择的唯一来源；已有值时不重新探测或静默改写。
- 不直接执行 `git clone`、`git fetch`、`git pull`、`git switch` 或 `git checkout`。
- 不手工创建、合并或修改 `.nlab` 配置。
- 不读取、复现或补偿 CLI 内部生成算法。
- 可选 Mock 工作只读取 CLI 公开产物及业务资料，整理规则与编排；不接管 CLI 的仓库操作、配置、解析或生成算法。
- 不对相同错误自动重试；用户修复仓库关联或索引后，可以继续被阻断的生成。不额外运行 typecheck、lint、测试或旧生成链。
- 使用 `--repo-url`、`--clone-dir`、`--contract-root` 或生成时的 `--branch` 前，先从对应 `--help` 确认参数存在。
  参数缺失时停止并提示升级 nlab-api；不得回退到手工 Git。

## Runner

先找到一个同时支持 `config --show` 和 `config --detect` 的命令入口：优先检查
`jt nlab-api config --help`，其次检查 `nlab-api config --help`。两者都不支持时停止并提示升级。

执行 `<config-command> config --show --project <frontend>`，按返回值选择 runner：

- `runner: jt`：确认 `jt` 可用，然后使用 `jt nlab-api`。
- `runner: nlab-api`：确认 `nlab-api` 可用，然后使用 `nlab-api`。
- runner 为空：执行 `<config-command> config --detect --project <frontend>`，再执行一次
  `config --show`，使用刚持久化的 runner。`config --detect` 负责优先选择 jt，再选择 nlab-api。
- `config --detect` 报告两个命令都不可用：停止并提示安装。
- 已配置的命令不可用：停止并报告，不切换另一个 runner。

写入成功后使用刚持久化的 runner。不得直接编辑 `.nlab/nlab-api.local.json`。

使用 `jt --version` 或 `nlab-api --version` 检查所选 runner。低于 2.2.3 时提示升级后继续，不自行切换 runner。

## 输入

- 前端项目目录；默认当前目录。
- 首次初始化时需要后端 Git URL 或已有本地路径。
- 可选：默认后端分支、clone 目标目录、`appName`、前端布局 `api|service`。
- 可选：后端提供的接口目录，可多个，路径相对后端仓库根目录。用户提供时使用 `--contract-root` 逐个传给 CLI；未提供时由 CLI 探测 `@ServiceContract` 接口目录。
- 生成时可选：只影响本次运行的后端分支。
- 可选：后端项目公共目录 `repositoriesRoot`。用户指定时通过 `--repositories-root <path>` 交给 CLI 持久化到本地配置。未指定时优先复用已保存目录，否则使用入口后端仓库的父目录；不为开启跨仓库发现额外询问目录。

缺少首次初始化所需的后端 URL 或路径时询问用户，不猜测仓库。

## 初始化

前端项目没有 `.nlab/nlab-api.config.json`，或用户明确要求更换共享 backend/default branch 时，
按用户提供的后端来源执行一种命令。不得手工 patch 配置。

Git URL：

```bash
<nlab-api> init --project <frontend> --repo-url <url>
```

本地路径：

```bash
<nlab-api> init --project <frontend> --repo-path <backend>
```

只在用户提供对应值时附加：

```text
--clone-dir <path>
--branch <branch>
--app-name <app-name>
--contract-root <path>    # 多个目录可重复传入
--layout <api|service>
--repositories-root <path>
```

后续示例用 `<nlab-api>` 表示已选择的命令前缀：

```text
jt nlab-api
nlab-api
```

初始化失败时原样展示 CLI 错误，用户补充缺失或冲突输入前停止。

## 生成

配置存在且用户未要求更换共享 backend/default branch 时执行：

```bash
<nlab-api> generate --project <frontend>
```

用户指定本次分支时附加：

```text
--branch <branch>
```

该参数不修改团队默认分支。CLI 负责锁定后端 checkout、切换分支、fast-forward 更新和
CodeGraph 同步。

CLI 先列出 `contractRoots` 中的接口方法，再查询 ZGateway。只有查到对应 HTTP 路径的方法才进入 Discover、契约和枚举分析；未匹配的方法直接舍弃。首参是否属于 `com.zhuanzhuan.arch.zgateway.support` 包不再决定入口身份。网关查询失败，或全部候选都未匹配时，生成失败，不保留占位路径。`appName` 用于网关查询，初始化时应与后端网关配置的服务名一致。

跨仓库发现默认开启，新项目和未配置 discovery 的旧项目都适用。直接执行上述 generate，CLI 自动确定并保存公共目录，每次在写入生成物之前执行 Discover；不需要 Skill 额外开启或重复扫描。
只有用户指定公共目录时才附加 `--repositories-root <path>`，使用前从 `generate --help` 确认参数存在。`generate --offline` 跳过网关查询，必须已有 `.nlab/contract-ir.json`，其中路由须匹配当前后端的 appName、分支和提交；首次生成需要在线查询网关。

## Discover 与目标分支

- 每个仓库独立维护 CodeGraph 索引，由 CLI 执行 init/sync。
- CLI 每次在线运行都重试不可用服务。缺仓库时查询 SIC 并 clone；已有依赖仓库按目标分支执行 fast-forward 更新，不丢弃本地修改。
- 依赖分支未指定时使用 `master`。用户指定分支时，先确认命令帮助，然后执行：

```bash
<nlab-api> discover --project <frontend> --service-branch <service>=<branch>
```

- 分支保存在 `discovery.services`；Skill 不手工修改配置。
- 不再询问或记录允许缺失。旧 `allowMissing` 字段由 CLI 忽略并在刷新配置时移除。
- 获取失败时 CLI 记录 `acquisition-failed`，继续生成可分析部分。展示失败服务、目标分支及错误，不把获取失败等同于无权限，不宣称结果完整。
- 下次运行重新尝试，不缓存跳过决定。`discover --offline` 跳过网络获取与网关查询，同样要求已有与当前后端目标匹配的契约 IR 路由。
- 退出码 `2` 仍表示关联歧义或索引阻断。直接 discover 的 stdout JSON 使用 `blockingServices`、`unavailableServices` 和 `acquisitions.<service>`；generate 则从 `.nlab/generate-report.json` 的 `stages.discovery` 读取这些字段。
- `unavailableServices` 只列本次源码不可用的服务名；目标分支、状态和错误分别读取 `acquisitions.<service>` 的 `branch`、`status`、`error`。无法核实的字段保持开放类型。

## 枚举结果

- 枚举判断由 CLI 的固定代码执行，Skill 不用 AI 二次推断或补生成枚举。
- 只生成已确认与接口字段关联的后端枚举主值。`name`、`desc`、`color`、`buttonType` 等附属值保持原始字段类型，不生成子枚举或映射表。
- 注释、`@see`、`@link` 仅提供候选；找到候选枚举类不代表关联成立。读取报告及 `enumCandidate` 中的 `verified`、`conflict`、`unverified`、`ignored`，以实际代码证据为准。未核实的候选保持 `number` / `string`；注释冲突不覆盖已确认的源码主值。
- 枚举归属与值域闭合分开解释：`closed` 表示已证明完整字段值域；`known` 配合 `enumAssociated: true` 可以确认业务枚举归属，不代表已证明数据库全部取值。生成还要求 `primaryEnumValue: true`，不能只凭 `knownValues` 判断会生成或收窄类型；显式 null 分支保留可空类型。
- 汇报实际生成的主值枚举、已确认来源和未解决链路；不把找到仓库、发现枚举类或附属属性算成新增主值枚举。
- 升级后旧注释枚举、附属枚举可能被删除。按生成报告说明需要迁移的业务引用，不恢复伪枚举、不把生成成功说成业务项目类型检查通过。

## 结果

- 保留 CLI 原始进度。
- 成功后展示 stdout 最终 JSON 和 `<frontend>/.nlab/generate-report.json`。
- `diagnostics` 不自动把成功改判为失败。
- 失败时展示原始错误和已有报告；退出码 `124` 明确说明超时。
- 网关查询失败、没有匹配路由或离线路由缓存不匹配时，报告失败原因；不要把这些情况解释为可继续生成的 warning。
- Discover 退出码 `2` 不是生成成功；获取失败警告不自动改判生成失败。

## 可选：Mock 规则与生成编排

仅当用户要求 Mock、场景生成或检查时，加载 [Mock 分档参考](references/mock-generation.md)。
普通初始化和接口生成不加载该参考，不自动追加 Mock 工作。

这是接口生成后的可选步骤；已有公开契约产物时可直接使用，无需重新初始化或同步后端。
先固化规则与覆盖说明，再按已核实的工具能力编排生成。正式 standalone 的 `nlab-api mock` 与可选 generate 阶段复用共享生成器；JT 兼容入口仍尊重 runner 配置。已明确开发约定的项目按参考中的简单 Whistle 文件映射执行；不为尚未出现的 GET 参数需求增加过滤和兼容。具体执行能力仍以实际命令帮助及运行结果为准。
