---
name: nlab-backend-bridge
metadata:
  version: 2.2.0
  opencode/autoinvoke: "false"
  invocation: manual
description:
  仅供用户显式调用 `$nlab-backend-bridge` 或客户端对应的手动 Skill 命令。按项目本地 config 选择 `jt nlab-api` 或 standalone `nlab-api`，初始化或生成 Java 后端契约、OpenAPI、TypeScript 类型和 API；config 为空时优先探测 jt 并持久化选择。Rust CLI 独占仓库 clone、分支切换、配置和生成逻辑。本 Skill 只收集缺失输入、触发命令、反馈报告。不是通用 Git、接口调试、代理、网关查询或 zapi 工具。
allowed-tools:
  - Bash
  - Read
  - AskUserQuestion
  - request_user_input
---

# nlab-backend-bridge

调用 `nlab-api`，将 Java 后端契约同步到前端项目。

## 边界

- 仅在用户显式调用本 Skill 时执行。
- runner config 是命令选择的唯一来源；已有值时不重新探测或静默改写。
- 不直接执行 `git clone`、`git fetch`、`git pull`、`git switch` 或 `git checkout`。
- 不手工创建、合并或修改 `.nlab` 配置。
- 不读取、复现或补偿 CLI 内部生成算法。
- 不自动重试，不额外运行 typecheck、lint、测试或旧生成链。
- 使用 `--repo-url`、`--clone-dir` 或生成时的 `--branch` 前，先从对应 `--help` 确认参数存在。
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

## 输入

- 前端项目目录；默认当前目录。
- 首次初始化时需要后端 Git URL 或已有本地路径。
- 可选：默认后端分支、clone 目标目录、`appName`、前端布局 `api|service`。
- 生成时可选：只影响本次运行的后端分支。

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
--layout <api|service>
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

## 结果

- 保留 CLI 原始进度。
- 成功后展示 stdout 最终 JSON 和 `<frontend>/.nlab/generate-report.json`。
- `diagnostics` 不自动把成功改判为失败。
- 失败时展示原始错误和已有报告；退出码 `124` 明确说明超时。
