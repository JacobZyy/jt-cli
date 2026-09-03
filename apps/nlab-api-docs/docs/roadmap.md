# Roadmap

本页同步仓库根目录 [`TODO.md`](https://github.com/JacobZyy/jt-cli/blob/main/TODO.md)。当前只剩一条主线：跨仓库枚举溯源。

## 已完成

### Rust-first Contract IR

- Facade operation 作为契约根。
- CodeGraph + Tree-sitter 分析调用链和值来源。
- OpenAPI、API、types 和 enums 从同一 Contract IR 生成。
- `closed`、`known`、`external`、`unresolved` 保留语义确定性。

### Standalone CLI

- `jt nlab-api` 与 standalone `nlab-api` 复用同一 Rust library。
- macOS Apple Silicon 预编译分发。
- 安装器、SHA-256 校验、版本检查和 self-update。
- 项目本地 runner 配置。

### afterGenerate

- 支持多个项目自有命令。
- 在前端项目根目录执行。
- 提供精确生成文件清单。
- 记录耗时和退出码；非零立即终止。

## 当前 TODO：跨仓库枚举溯源

本服务内的分析会在 RPC 边界停止。下一阶段允许在明确配置下继续读取兄弟仓库已有的 CodeGraph 索引。

目标：

- 通过配置的 repositories root 定位内部 RPC 目标。
- 读取目标仓库已有 CodeGraph SQLite，不自动扫描任意目录。
- 记录源仓库和目标仓库 commit 身份。
- 沿目标服务执行图继续 enum provenance，也就是字段值来源证据。
- 把跨仓库证据合并回原 operation 的字段身份。

证据优先级保持不变：

1. 完整调用链枚举。
2. 完整 Javadoc-linked enum。
3. 注释或注解中的显式值。
4. 原始 scalar。

安全边界：

- 目标仓库缺失时停止。
- RPC 目标歧义时停止。
- commit 或符号冲突时停止。
- 目标索引不完整或版本不支持时停止。
- 任一链路未闭合时，不得输出 `closed`。

## 验收标准

跨仓库能力完成前，至少满足：

- 被追踪的兄弟仓库及其 CodeGraph 索引只读。
- 每个仓库 commit 可追溯。
- 相同输入产生确定性结果。
- 单仓库和跨仓库证据冲突时降级，不覆盖。
- 缺失、歧义、循环、预算耗尽均有明确诊断。
- 不要求用户把所有后端仓库放进一个 workspace。

## 不做

- 不自动 clone 未配置仓库。
- 不扫描用户整个磁盘寻找服务。
- 不把 RPC 返回值附近的文案当成枚举证据。
- 不因跨仓库失败阻断单仓库核心契约生成。

完整待办见仓库根目录 [`TODO.md`](https://github.com/JacobZyy/jt-cli/blob/main/TODO.md)。
