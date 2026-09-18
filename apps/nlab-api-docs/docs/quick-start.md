# Quick Start

本指南使用 standalone `nlab-api`。它是团队默认入口，不需要 Rust 或 Cargo。

已有接口契约、需要固定 Mock 时，见[语义化 Mock](./mock.md)。

## 前置条件

- macOS Apple Silicon。
- `curl`、`tar`、`shasum`。
- `git`。
- `codegraph`。
- 一个前端项目。
- 一个包含 NLab Java Facade 的后端仓库 URL 或本地 checkout。

ZGateway 需要公司网络或 VPN，但它不是核心类型生成的前置条件。

## 准备 CodeGraph

团队当前开发环境通过 Vite+ CLI 暴露 `codegraph`。先验证：

```bash
vp --version
codegraph --version
```

若命令不存在，安装或更新团队批准的 Vite+ 版本。Vite+ 公共安装入口见 [Getting Started](https://viteplus.dev/guide/)：

```bash
curl -fsSL https://vite.plus | bash
```

重新打开 shell 后再次执行 `codegraph --version`。nlab-api 会负责 `codegraph init` 或 `sync`，但不会安装 CodeGraph。

本文验证环境为 `codegraph 1.4.1`。当前 CLI 尚未声明更低的兼容版本；团队使用前应锁定能提供该命令的 Vite+ channel。

## 安装

下载安装脚本，再执行：

```bash
curl -fsSL \
  https://raw.githubusercontent.com/JacobZyy/jt-cli/main/install-nlab-api.sh \
  -o /tmp/install-nlab-api.sh

sh /tmp/install-nlab-api.sh
```

默认安装路径：

```text
~/.local/bin/nlab-api
```

若命令不在 `PATH`：

```bash
export PATH="$HOME/.local/bin:$PATH"
```

验证安装：

```bash
nlab-api --version
```

安装器会校验 SHA-256 和二进制版本，并写入 self-update ownership marker。marker 只证明安装目录曾由安装器接管，不绑定二进制 hash。没有 marker 的手工安装不会被自动覆盖；若手工替换已托管目录中的二进制但保留 marker，后续 updater 仍可覆盖它。

## 初始化项目

使用 Git URL 时，CLI 负责 clone。`--clone-dir` 可省略；默认目录位于
`~/.local/share/nlab-api/repos/`，名称由仓库名和 origin 摘要组成：

```bash
nlab-api init \
  --project /path/to/frontend \
  --repo-url git@example.com:team/backend.git \
  --clone-dir /path/to/backend \
  --branch feature-branch \
  --app-name service_name
```

已有本地 checkout 时：

```bash
nlab-api init \
  --project /path/to/frontend \
  --repo-path /path/to/backend \
  --branch feature-branch \
  --app-name service_name
```

参数含义：

- `--project`：前端项目根目录。
- `--repo-url`：团队可共享的 Git origin；与 `--repo-path` 二选一。
- `--repo-path`：本机已有后端 checkout；与 `--repo-url` 二选一。
- `--clone-dir`：`--repo-url` 的可选本地目标目录。
- `--branch`：团队默认后端分支；省略时使用 clone 或本地仓库当前分支。
- `--app-name`：服务身份和 placeholder path 使用的名称；省略时使用后端目录名。
- `--layout api|service`：强制输出目录族；省略时根据现有 `src/api` 或 `src/service` 检测。
- `--timeout-seconds`：包含 clone 和更新的整体超时，默认 1200 秒。

`init` 会：

- 检测 Vite、TypeScript 和请求适配器。
- 识别 `src/api` 或 `src/service` 输出布局。
- clone 或复用后端仓库，校验 origin，安全切换并 fast-forward 目标分支。
- 将团队配置写入 `.nlab/nlab-api.config.json`。
- 将本机 `repoPath` 写入 `.nlab/nlab-api.local.json`，并加入 `.gitignore`。
- 发现 Vite 配置时，默认启用额外 alias 并幂等补充 Vite、TypeScript 和必要的测试配置。
- 非 Vite 项目默认关闭额外 alias，生成代码直接使用现有 `@/`，不改构建、TypeScript 或测试配置。

开关保存于 `.nlab/nlab-api.config.json` 的 `frontend.aliases.enabled`。Vite 项目也可以手动关闭：

```json
{
  "frontend": {
    "aliases": {
      "enabled": false
    }
  }
}
```

上面仅展示需要修改的开关，保留配置中的其他字段。旧配置未写该字段时保持启用行为；重新初始化非 Vite 项目会写入 `false`。关闭后，生成目录必须位于 `frontend.sourceRoot` 内，项目自身负责已有 `@/` 的解析。

提交前先检查生成配置：

- `backend.repository`、`branch`、`appName`。
- 自动发现的 `contractRoots`。
- `frontend.request.module`、`export`、`responseMode`。
- `frontend.layout` 和 aliases。
- `gateway`、`migration`、`mock`、`afterGenerate`。

配置范围：

| 文件 | 归属 | 内容 |
|---|---|---|
| `.nlab/nlab-api.config.json` | 团队共享，提交 Git | repository、默认 branch、契约和生成规则 |
| `.nlab/nlab-api.local.json` | 本机，Git 忽略 | 绝对 `repoPath`、可选 `runner: jt|nlab-api` |

字段按明确职责解析，不做任意 JSON 深合并：本地 `repoPath` 优先于 CLI 托管目录，命令行
`--branch` 只覆盖本次运行，其他生成规则来自团队配置。

版本 1 配置中的 `backend.repoPath` 和旧 `.nlab/cli.local.json` 仍可读取；新 `init`
只写分层后的格式。已有项目需要显式重跑一次 `init` 才会迁移文件，不会在 `generate` 时改写配置。

## 生成契约

```bash
nlab-api generate --project /path/to/frontend
```

临时同步另一个分支，不修改团队默认配置：

```bash
nlab-api generate --project /path/to/frontend --branch another-branch
```

下次不传 `--branch` 时，CLI 自动切回团队配置分支。

`generate` 按顺序执行：

1. 获取后端仓库锁；缺失时根据 repository URL clone。
2. 拒绝后端 tracked 改动；不删除或覆盖 untracked 文件。
3. 切换配置分支或本次 `--branch`，再执行 `git pull --ff-only`。
4. `codegraph init` 或 `codegraph sync`。
5. 解析 Facade operation、请求类型、响应类型和 DTO 图。
6. 分析调用链与字段值来源。
7. 尝试补全 ZGateway 路由。
8. 生成 OpenAPI、API、types 和 enums。
9. 执行配置的 `afterGenerate`。
10. 执行可证明的 migration；按配置生成 Mock。
11. 原子提升产物并写入报告。

核心产物：

```text
.nlab/
├── nlab-api.config.json
├── contract-ir.json
├── openapi.json
├── frontend-manifest.json
└── generate-report.json
```

实际 API、types 和 enums 目录由项目配置决定。

## 读取结果

成功状态：

- `complete`：全部阶段完成，无诊断。
- `complete-with-warnings`：核心产物完成；路由 placeholder、开放枚举或其他保守降级已记录。

进程退出码：

- `0`：`complete` 或 `complete-with-warnings`。
- `1`：配置、解析、生成、hook 或写入失败。
- `124`：超过整体 deadline。

自动版本查询失败时，nlab-api 打印 warning 并继续当前命令。已经发现新版本后，下载、checksum、ownership 校验、替换或重新执行失败会返回非零；显式 `nlab-api update` 失败返回 `1`。

常见诊断：

- `GATEWAY_QUERY_FAILED`：内网查询失败，保留 placeholder。
- `GATEWAY_ROUTE_NOT_FOUND`：指定 operation 没有查到真实路由。
- `ENUM_KNOWN`：找到部分 enum 证据，值域保持开放。
- `ENUM_EXTERNAL`：值来源终止于 RPC 或 Database。
- `MIGRATION_REFERENCE_RETAINED`：迁移无法唯一决定，原引用保留。

CI 应先以退出码判断成功，再按项目要求检查 diagnostics。`complete-with-warnings` 不是进程失败；需要真实路由或严格枚举的项目可以把对应 warning 作为自己的门禁。

生成失败时，先看：

```text
.nlab/generate-report.json
```

不要把所有 warning 当成失败。ZGateway 不可用、外部值来源或无法闭合的枚举会保留诊断，但不一定阻断核心产物。

## 更新

普通命令启动前会检查最新 ready release。发现新版本时，nlab-api 下载完整 archive、校验、原子替换，再重新执行原命令。

手动检查和更新：

```bash
nlab-api update --check
nlab-api update
nlab-api upgrade
```

`upgrade` 是 `update` 的别名。显式更新会通过 Skill Manager 同步已安装的
`nlab-backend-bridge`，即使二进制已经是最新版本也会同步。`--check` 只检查，不更新 Skill。
普通命令触发二进制自动升级时，也会尝试同步 Skill；没有新二进制时不会每次启动都更新 Skill。
`jt upgrade` 不执行这一步。

同步优先使用 `~/.skills-manager/bin/skills-manager-cli`，其次使用 PATH 中的同名命令。
CLI 只委托 Skill Manager 更新这一项，不自行修改 Skill 文件、切换整个技能库分支或更新其他技能。
没有安装 Skill Manager、没有安装该 Skill，或 Skill 仍登记为本地导入时，会说明原因并跳过同步。

本地导入的 Skill 需要先在 Skill Manager 中关联实际远程来源。例如，Skill 位于仓库的同名子目录时：

```bash
~/.skills-manager/bin/skills-manager-cli skills set-source nlab-backend-bridge \
  --git-url https://github.com/your-org/skills.git \
  --branch main --subpath nlab-backend-bridge
```

来源、认证、文件更新及部署由 Skill Manager 管理；nlab-api 不公开或内置你的私有技能仓库内容。
显式更新中 Skill Manager 失败会返回非零，并说明二进制更新已经完成，重跑 `nlab-api update`
即可重试 Skill 同步，不会为此回滚二进制。自动升级时的 Skill 同步失败则打印警告，继续原命令。

从尚未包含此能力的旧版升级时，首次升级仍由旧 updater 执行；升级完成后再运行一次
`nlab-api update` 同步 Skill，之后的升级会自动同步。

自动更新只替换安装器写入 ownership marker 的二进制：

- marker 存在：自动更新和显式 `update` 可替换。
- marker 缺失：自动检查提示并继续当前版本；显式 `update` 返回错误。
- 处理方式：重新执行安装脚本，让安装器接管该二进制。

离线或可重复执行：

```bash
nlab-api --no-update generate --project /path/to/frontend
```

## Runner 配置

两个入口写入同一份本地配置：

```bash
jt nlab-api config --runner jt --project /path/to/frontend
nlab-api config --runner nlab-api --project /path/to/frontend
jt nlab-api config --detect --project /path/to/frontend
```

读取当前配置：

```bash
jt nlab-api config --show --project /path/to/frontend
nlab-api config --show --project /path/to/frontend
```

runner 保存在 `.nlab/nlab-api.local.json`，并自动加入目标项目 `.gitignore`。
`nlab-backend-bridge` 始终使用已配置 runner。runner 为空时调用 `config --detect`；该命令先检测
`jt`，再检测 `nlab-api`，持久化第一个可用命令。已有配置对应的命令不可用时停止，不静默回退。

清除 runner；下次 Skill 调用时重新检测：

```bash
jt nlab-api config --unset --project /path/to/frontend
```

## 跨仓库准备情况检查

启用跨仓库分析时，给 generate 或 init 提供后端公共目录：

```bash
jt nlab-api generate \
  --project /path/to/frontend \
  --repositories-root /path/to/backend-projects
```

公共目录持久化到本地配置 `backend.repositoriesRoot`，不写入共享配置。
后续每次 generate 自动执行 Discover，不需要重复传目录或手动调用 discover。
旧项目没有配置 discovery 和公共目录时，保留原单仓库生成行为。

CLI 将发现结果保存到共享配置 `.nlab/nlab-api.config.json` 的 `discovery.services`：

```json
{
  "discovery": {
    "services": {
      "categoryr": {
        "status": "missing",
        "allowMissing": true,
        "interfaces": ["example.ICategoryService"]
      }
    }
  }
}
```

已解析的服务还记录 Git `repository` 地址。每次刷新保留用户决定；补上仓库后清除该服务的
`allowMissing`。不再被当前入口引用的服务标记为 `unused`，不会继续阻断。
仅找到类型、没有 RPC 绑定的线索以 `interface:<完整类名>` 记录，避免伪造服务名。

缺少仓库且未获允许时，generate 退出码为 `2`，报告 `status: blocked`，并且不覆盖已有生成物。
查看 `.nlab/generate-report.json` 的 `stages.discovery.blockingServices`，补全仓库后重新生成。
只有明确决定不用补某个服务时，执行：

```bash
jt nlab-api discover --project /path/to/frontend --allow-missing categoryr
```

可重复传 `--allow-missing`；不接受不存在于本次发现结果或并非缺失的服务。索引不可用必须修复，不能用允许缺失掩盖。
后续 generate 不再询问已允许的服务，但仍会检查它是否已经补上。允许缺失会出现在生成报告的警告中。

单独执行 discover 会更新发现配置并输出 JSON，不生成接口代码。两个入口遵守相同 runner 配置。
默认从 `backend.contractRoots` 中所有接口方法开始。单独 discover 只分析某个文件时，可追加：

```bash
--entry contract/src/main/java/example/IExampleFacade.java
```

`--entry` 相对于当前配置的后端仓库，也支持仓库内的绝对路径。工具读取当前 checkout；
实际分支与生成配置不同时会报告，不切换分支。

公共目录的直接子目录作为仓库清单。优先读取各仓库 `.codegraph/codegraph.db`；
缺失或不可用时，从公共目录的统一索引中提取对应仓库。generate 会先同步公共目录索引，
分析时优先使用刚同步的公共索引；写入前检查所读 Java 源码是否发生变化。

结果以 JSON 输出到 stdout，可重定向到自己选择的报告文件。报告包含：

- `entries`：接口入口。
- `repositories`：本地仓库路径、Git origin、分支、commit、脏状态、SCF 服务名、索引情况及是否参与分析。
- `calls`：实际到达的跨服务调用点、代表性本地调用链、SCF 配置依据、候选仓库及继续搜索所需的完整接口名和服务名。
- `warnings`、`unresolvedLocalCalls`：分析断点及无法解析的本地调用数量；后者也包括未索引的库方法和生成的 getter。
- `blockingServices`、`allowedMissingServices`：尚需处理的服务及本次按明确决定允许缺失的服务。

目前识别 `src/main/resources` 下 XML 中声明的 SCF `applicationName`、
`references serviceName` 和 `reference interface`。只有服务绑定与仓库声明匹配，且目标
接口的方法名及参数数量唯一时，才继续遍历。没有绑定、多个候选、方法重载、索引缺失都保留断点。
Java 注解、动态路由、运行时注册中心和 Maven 版本解析尚未接入；不会通过名称相似推定仓库。

| `calls[].status` | 含义 |
| --- | --- |
| `source-matched` | 找到唯一的本地源码候选，继续追踪 |
| `missing-source` | 当前仓库清单中没有匹配来源，不代表 Git 平台上不存在 |
| `index-unavailable` | 找到服务对应仓库，但没有可用索引 |
| `ambiguous` | 多个服务绑定或仓库候选，不能唯一确定 |
| `unbound-candidate` | 存在接口源码，但缺少确认服务归属的绑定 |
| `method-unresolved` | 目标方法缺失或存在歧义 |
| `depth-limit` | 已达到跨仓库遍历深度上限 |

`--max-depth` 默认为 3，允许 1–16；本地调用不消耗跨仓库深度。调用循环会去重。
每个仓库最多遍历 25000 个方法，达到上限会报告。调用链保留一条代表路径，不是完整的逐入口调用矩阵。

generate 在通过 Discover 后，将已匹配仓库加入同一次源码分析，复用原枚举提取与 TypeScript/OpenAPI 生成器。
节点、源码路径和缓存按仓库隔离；重复完整类型名不会任意选取。请求字段继续追踪必经校验，
响应字段支持 DTO getter/setter 复制及枚举反查来源。`closed` 才收窄字段类型；`knownValues`
保存已核实枚举成员并生成独立枚举文件，字段保留 `number` / `string`。后者不是完整取值约束。

单独 discover 不同步索引，因此其新鲜度标记为 `not-verified`；它会写发现配置，但不会 clone、fetch、
切换分支或改生成物，也不会触发 standalone 自动升级。generate 的发现报告说明本地源码情况，
不证明线上部署、Maven 版本一致性或动态服务绑定。缺失仓库地址仍需补全，尚未接入远程 Git 平台检索。

在隔离 Worktree 中测试或只使用当前源码时，可给 init / generate 传 `--offline`。
此模式不执行 Git clone、切分支或 pull；generate 仍同步本地索引并执行配置的生成后命令，跳过 Gateway 查询。
分支参数必须匹配当前 checkout，生成物中的 placeholder 路由不能当作已验证的线上路由。

## 常见问题

### `unsupported platform`

预编译分发目前只支持 macOS Apple Silicon。

### 找不到 `codegraph`

确认 `codegraph` 在当前 shell 的 `PATH` 中。nlab-api 不会替你安装或升级 CodeGraph。

### 后端仓库准备失败

CLI 不会 stash、reset 或覆盖本地文件。先处理 tracked 修改、错误 origin、分叉、
并发运行或无法 fast-forward 的目标分支，再重试。

### ZGateway 查询失败

连接公司网络或 VPN 可补全真实路由。无法连接时，nlab-api 保留 placeholder 和 warning；不要手工猜测 path。
