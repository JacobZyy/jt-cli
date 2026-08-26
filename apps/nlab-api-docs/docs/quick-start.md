# Quick Start

本指南使用 standalone `nlab-api`。它是团队默认入口，不需要 Rust 或 Cargo。

::: warning 发布状态
仓库尚未发布包含 standalone nlab-api asset 的版本。以下安装命令在首个 nlab-api GitHub Release 完成后生效。
:::

## 前置条件

- macOS Apple Silicon。
- `curl`、`tar`、`shasum`。
- `git`。
- `codegraph`。
- 一个前端项目。
- 一个包含 NLab Java Facade 的后端仓库。

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

在真实前端项目和后端仓库上执行：

```bash
nlab-api init \
  --project /path/to/frontend \
  --repo-path /path/to/backend \
  --branch feature-branch \
  --app-name service_name
```

参数含义：

- `--project`：前端项目根目录。
- `--repo-path`：已 checkout 的后端仓库。
- `--branch`：需要同步的后端目标分支；省略时使用后端当前分支。
- `--app-name`：服务身份和 placeholder path 使用的名称；省略时使用后端目录名。
- `--layout api|service`：强制输出目录族；省略时根据现有 `src/api` 或 `src/service` 检测。

`init` 会：

- 检测 Vite、TypeScript 和请求适配器。
- 识别 `src/api` 或 `src/service` 输出布局。
- 写入 `.nlab/nlab-api.config.json`。
- 幂等补充构建工具和 TypeScript alias。

提交前先检查生成配置：

- `backend.repoPath`、`branch`、`appName`。
- 自动发现的 `contractRoots`。
- `frontend.request.module`、`export`、`responseMode`。
- `frontend.layout` 和 aliases。
- `gateway`、`migration`、`mock`、`afterGenerate`。

## 生成契约

```bash
nlab-api generate --project /path/to/frontend
```

`generate` 按顺序执行：

1. `git pull --ff-only` 更新配置分支。
2. `codegraph init` 或 `codegraph sync`。
3. 解析 Facade operation、请求类型、响应类型和 DTO 图。
4. 分析调用链与字段值来源。
5. 尝试补全 ZGateway 路由。
6. 生成 OpenAPI、API、types 和 enums。
7. 执行配置的 `afterGenerate`。
8. 执行可证明的 migration；按配置生成 Mock。
9. 原子提升产物并写入报告。

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

## 使用 jt 内嵌实现

已安装 `jt` 时，可以为单个项目切换 runner：

```bash
jt nlab-api config --runner jt --project /path/to/frontend
```

之后：

```bash
jt nlab-api init ...
jt nlab-api generate --project /path/to/frontend
```

会使用 jt 内嵌的 nlab-api。配置保存在 `.nlab/cli.local.json`，并自动加入目标项目 `.gitignore`。standalone `nlab-api` 不读取它。

恢复默认 standalone：

```bash
jt nlab-api config --unset --project /path/to/frontend
```

## 常见问题

### `unsupported platform`

预编译分发目前只支持 macOS Apple Silicon。

### 找不到 `codegraph`

确认 `codegraph` 在当前 shell 的 `PATH` 中。nlab-api 不会替你安装或升级 CodeGraph。

### `git pull --ff-only` 失败

后端仓库必须处于可 fast-forward 的目标分支。先处理本地提交、分叉或冲突，再重试。

### ZGateway 查询失败

连接公司网络或 VPN 可补全真实路由。无法连接时，nlab-api 保留 placeholder 和 warning；不要手工猜测 path。
