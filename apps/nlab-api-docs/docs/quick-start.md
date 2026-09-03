# Quick Start

本指南使用 standalone `nlab-api`。它是团队默认入口，不需要 Rust 或 Cargo。

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
- 幂等补充构建工具和 TypeScript alias。

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
```

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
