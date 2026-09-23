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

首次识别接口需要查询 ZGateway；连接公司网络或 VPN。已有匹配的生成结果时，`--offline` 可复用其路由。

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
  --app-name service_name \
  --contract-root contract/src/main/java
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
- `--app-name`：网关查询使用的服务身份；省略时使用后端目录名。
- `--contract-root`：后端提供的接口目录，相对仓库根目录；多个目录可重复传入。省略时尝试从 `@ServiceContract` 接口目录探测。
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
- 后端提供或自动探测的 `contractRoots`。
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
4. `codegraph init` 或 `codegraph sync`，读取 `contractRoots` 中的接口方法候选。
5. 查询 ZGateway；只保留具有对应 HTTP 路径的方法。查询失败时停止生成。
6. 从保留的方法出发，发现关联仓库并解析请求、响应和 DTO。
7. 分析调用链、枚举与字段值来源。
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
- `complete-with-warnings`：核心产物完成；开放枚举或其他保守降级已记录。

进程退出码：

- `0`：`complete` 或 `complete-with-warnings`。
- `1`：配置、解析、生成、hook 或写入失败。
- `124`：超过整体 deadline。

自动版本查询失败时，nlab-api 打印 warning 并继续当前命令。已经发现新版本后，下载、checksum、ownership 校验、替换或重新执行失败会返回非零；显式 `nlab-api update` 失败返回 `1`。

常见诊断：

- 网关查询失败：命令返回非零；检查网络和 `zzcli` 权限。
- 网关未匹配：候选方法被舍弃；全部未匹配时命令返回非零。
- `ENUM_KNOWN`：找到部分 enum 证据，值域保持开放。
- `ENUM_EXTERNAL`：值来源终止于 RPC 或 Database。
- `MIGRATION_REFERENCE_RETAINED`：迁移无法唯一决定，原引用保留。

CI 应先以退出码判断成功，再按项目要求检查 diagnostics。`complete-with-warnings` 不是进程失败；需要严格枚举的项目可以把对应 warning 作为自己的门禁。

生成失败时，先看：

```text
.nlab/generate-report.json
```

不要把所有 warning 当成失败。外部值来源或无法闭合的枚举会保留诊断，但不一定阻断核心产物。

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

跨仓库发现默认开启，新项目和未配置 discovery 的旧项目都适用。
默认使用入口后端仓库的父目录，优先复用本地已保存的公共目录。
需要覆盖目录时，给 generate、init 或 discover 提供：

```bash
jt nlab-api generate \
  --project /path/to/frontend \
  --repositories-root /path/to/backend-projects
```

公共目录持久化到本地配置 `backend.repositoriesRoot`，不写入共享配置。
init 会记录默认目录；旧项目首次 generate 时自动补齐，不需要重新初始化。
每次 generate 自动执行 Discover，不需要重复传目录或手动调用 discover。
例如入口仓库为 `/path/to/backend-projects/service-a` 时，默认扫描同级仓库，缺失依赖也 clone 到 `/path/to/backend-projects`。
`--offline` 跳过联网获取，使用与当前后端提交匹配的已生成路由，并执行本地跨仓库发现。

CLI 将发现结果保存到共享配置 `.nlab/nlab-api.config.json` 的 `discovery.services`：

```json
{
  "discovery": {
    "services": {
      "categoryr": {
        "status": "missing",
        "branch": "master",
        "interfaces": ["example.ICategoryService"]
      }
    }
  }
}
```

已解析的服务还记录 Git `repository` 地址及本次状态。依赖仓库的目标分支保存在
`discovery.services.<service>.branch`，未指定时使用 `master`，不会改用远端默认分支。
不再缓存允许缺失决定；旧 `allowMissing` 字段不参与判断，配置刷新时移除。
仅找到类型、没有 RPC 绑定的线索以 `interface:<完整类名>` 记录，避免伪造服务名。

指定依赖分支：

```bash
jt nlab-api discover --project /path/to/frontend --service-branch categoryr=feature/example
```

可重复传 `--service-branch`。每次在线 Discover 都重新尝试获取缺失仓库，并对已有依赖仓库
切换到目标分支、执行 fast-forward 更新。认证、权限、分支不存在或 clone/pull 失败只记录本次
`acquisition-failed`，继续生成可分析部分；失败仓库的旧索引不参与本次分析。
下次在线运行会再次尝试，不要求用户记录跳过决定。工作区有修改时不丢弃修改。
索引本身不可用或仓库关联歧义仍退出 `2`，且不覆盖旧生成物。

单独执行 discover 会更新发现配置并输出 JSON，不生成接口代码。两个入口遵守相同 runner 配置。
默认从 `backend.contractRoots` 中匹配网关路径的接口方法开始。单独 discover 只分析某个文件时，可追加：

```bash
--entry contract/src/main/java/example/IExampleFacade.java
```

`--entry` 相对于当前配置的后端仓库，也支持仓库内的绝对路径。工具读取当前 checkout；
实际分支与生成配置不同时会报告，不切换分支。

公共目录的直接子目录作为仓库清单。每个仓库独立维护 `.codegraph/codegraph.db`，
Discover 自动执行 `codegraph init --yes` 或 `codegraph sync`，不再读取或同步公共目录索引。
跨仓库分析仍会在内存中关联各仓库图；写入前检查所读 Java 源码是否发生变化。

本地缺失的服务先通过 `zzcli sic get-cluster-info-by-app-name` 查集群，再通过
`get-cluster-info-with-group` 获取 `beetleInfo.groupName/projectName`，组成公司 GitLab SSH 地址。
CLI 自动 clone 目标分支到公共目录，初始化索引，再继续追踪新仓库的依赖；同名目录冲突不覆盖。
`discover --offline` 和 `generate --offline` 跳过 SIC、clone、pull 和网关查询，复用现有 `.nlab/contract-ir.json` 中与后端目标匹配的路由，并同步本地仓库索引。首次生成需要在线查询网关。

结果以 JSON 输出到 stdout，可重定向到自己选择的报告文件。报告包含：

- `entries`：接口入口。
- `repositories`：本地仓库路径、Git origin、分支、commit、脏状态、SCF 服务名、索引情况及是否参与分析。
- `calls`：实际到达的跨服务调用点、代表性本地调用链、SCF 配置依据、候选仓库及继续搜索所需的完整接口名和服务名。
- `warnings`、`unresolvedLocalCalls`：分析断点及无法解析的本地调用数量；后者也包括未索引的库方法和生成的 getter。
- `acquisitions`：本次自动获取的仓库地址、获取状态和失败原因。
- `blockingServices`、`unavailableServices`：存在关联或索引阻断的服务，以及本次源码不可用但不阻断生成的服务。

目前识别 `src/main/resources` 下 XML 中声明的 SCF `applicationName`、
`references serviceName` 和 `reference interface`。服务归属由仓库声明或已核实的 SIC 仓库关联确定；只有归属唯一，且目标
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
响应字段支持 DTO getter/setter 复制及同值枚举反查。同一值用于 HTTP 字段与经过核实的枚举查找，即可建立枚举关联；查找结果可以用于名称、颜色、行为判断或其他用途，不限定为描述字段。

只生成后端枚举主值：名称、描述、颜色、按钮样式等附属属性保持原始字段类型，不生成子枚举或映射表。主值优先从 `@JsonValue`、直接返回字段的 `val()`、已证明的反查键识别；其余仅接受唯一明确的标识字段约定，多候选保持未确认。显式构造器按字段与形参赋值关系提取值，不使用字段声明顺序猜测。

关联与闭合证明分开记录：`closed` 表示已证明完整字段值域；`known` 加 `enumAssociated: true` 表示已确认业务枚举归属，但不宣称数据库值域已被证明。生成还必须满足 `primaryEnumValue: true`；操作级 TypeScript 类型和 OpenAPI 共用此判断。同一主值经 `val()`、getter 或直接字段访问时使用统一身份。显式 null 分支保留可空类型。

该判定仅沿当前接口的请求、响应及 RPC 调用链，不追查数据库所有写入路径。支持同值局部别名、多参数转发函数、DTO 复制和增强 for 循环中的枚举变量。不同对象、读取之间的值修改、冲突枚举或投影、未解析写入、算术变换以及枚举外默认值不建立类型关联。旧 IR 缺少主值确认标记时不会猜测收窄，重新 generate 可按新规则分析。`knownValues` 存在并不自动代表可关联。

DTO 来源追踪支持返回对象、参数转发、结果包装和 holder 的 getter/setter 中转；同一方法的多个调用实参无法证明为同一来源时保持开放。复杂集合映射、动态调用或未识别的包装保持原始类型，不再导出孤立枚举。字段类型调整后，业务代码中旧的宽泛类型、注释枚举和测试数据可能需要同步适配。

`@see` 只记录枚举候选及源码位置，不凭注释直接收窄字段；缺失、歧义或与实际代码冲突的引用保持可见，实际代码证据优先。嵌套 DTO 按外层类型的 import 解析。`static final` 字面量常量可参与字段值分析，但枚举外兜底仍阻止纯枚举关联。字符串注释中的裸数字状态示例不再自动当作字符串枚举值，明确的枚举声明和带引号的值仍可解析。

注释和链接得到的候选按接口、请求/响应及字段路径核实，并在 `enumCandidate` 和生成报告中记录 `verified`、`conflict`、`unverified` 或 `ignored`。即使候选类存在，也必须有独立的代码关联证据才能生成枚举；无法确认时保留 `number` / `string`。注释与源码成员不一致时记录差异，使用已确认的源码主值；这不宣称枚举的每个成员都会出现在该响应中。整个过程由固定 Rust 规则执行，不调用 AI。

单独 discover 不同步索引，因此其新鲜度标记为 `not-verified`；它会写发现配置，但不会 clone、fetch、
切换分支或改生成物，也不会触发 standalone 自动升级。generate 的发现报告说明本地源码情况，
不证明线上部署、Maven 版本一致性或动态服务绑定。缺失仓库地址仍需补全，尚未接入远程 Git 平台检索。

在隔离 Worktree 中测试或只使用当前源码时，可给 init / generate 传 `--offline`。
此模式不执行 Git clone、切分支或 pull；generate 仍同步本地索引并执行配置的生成后命令。分支参数必须匹配当前 checkout，已有 IR 的路由也必须匹配当前后端提交。

## 常见问题

### `unsupported platform`

预编译分发目前只支持 macOS Apple Silicon。

### 找不到 `codegraph`

确认 `codegraph` 在当前 shell 的 `PATH` 中。nlab-api 不会替你安装或升级 CodeGraph。

### 后端仓库准备失败

CLI 不会 stash、reset 或覆盖本地文件。先处理 tracked 修改、错误 origin、分叉、
并发运行或无法 fast-forward 的目标分支，再重试。

### ZGateway 查询失败

连接公司网络或 VPN，检查 `zzcli` 权限后重试。网关查询失败会停止入口识别；不会把未验证的方法生成为 HTTP API。
