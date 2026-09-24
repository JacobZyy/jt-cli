# Why nlab-api

nlab-api 解决的不是“把 Java 类型翻译成 TypeScript”这一件小事。它要把后端真实对外契约，稳定地交付给前端。

## 前端需要的不只是 DTO 结构

传统接口同步通常能拿到方法名、请求 DTO、响应 DTO。业务代码还依赖更多信息：

- 接口属于哪个 Facade operation。
- 嵌套 DTO、泛型、继承与内部类的准确身份。
- 字段在当前 operation 下可能出现的 wire value。
- 网关确认的 API 实际路由。
- 生成文件的所有权、迁移关系与失败报告。

只生成一个 OpenAPI 文件，再交给多段工具继续推导，会逐步丢失这些信息。nlab-api 因此把契约抽取、语义补全和前端产物生成放进同一条确定性流程。

## 原问题：注释不是完整事实

早期枚举生成依赖 Java 字段注释。注释可能缺值、过期，或者只描述部分业务状态。

`queryRecycleOrderList` 曾暴露典型问题：`actionCode` 的注释没有列出 `view_goods`，但后端实现会通过 `RecycleOrderActionEnum` 返回它。前端若按不完整注释生成封闭枚举，就不会处理真实值。

IDE 能沿调用链找到答案：

```text
IRecycleQueryFacade#queryRecycleOrderList
  RecycleOrderListQueryService#convertToRow
  RecycleOrderListQueryService#buildActionList
  buildButton(RecycleOrderActionEnum)
  BizButton.actionCode
```

nlab-api 把这类人工追踪变成可重复分析。

## 契约根节点为什么是 Facade

Facade 方法定义前端真正调用的公共 operation：

- 方法参数决定 Java 类型；ZGateway 的参数映射决定前端字段名及 query/body 位置。
- 方法返回值决定响应根类型。
- 注解与服务声明提供契约入口。

内部 Service 只属于实现。它可能包含上下文参数、内部对象、多个委托和非公开方法。把 Service 当作契约根，会把实现细节误当成前端 API。

因此，nlab-api 固定使用 Facade operation 作为根节点，再向两类关系图扩展。

## 两类图回答两个问题

### 类型图：接口有什么字段

```text
Facade operation
  request / response
  DTO fields
  nested DTO
  List<T> / Map<K, V>
  parent DTO
```

类型图处理 package、import、FQN、泛型、继承和内部类。它回答“契约结构是什么”。

### 执行图：字段值从哪里产生

```text
Facade operation
  implementation / Service / helper
  setter / constructor / assignment
  enum accessor / constant / RPC / Database
```

CodeGraph 提供调用和引用关系。Rust 读取 CodeGraph SQLite，再用 Tree-sitter 分析 Java 赋值表达式、分支和枚举成员。它回答“字段值如何产生”。本文所说的 enum provenance，就是这张执行图提供的值来源证据，不是第三张图。

两类图通过稳定字段身份连接：

```text
operationKey + schemaFqn + fieldPath
```

`operationKey` 不能省略。同一个 DTO 字段在不同 operation 中可能拥有不同值域；全局修改 DTO 会污染其他接口。

生成类型时，同一个 Java DTO 在各接口中的最终字段类型相同，就复用同一份 TypeScript 类型和 OpenAPI schema；未变化的嵌套 DTO 也会复用。只有枚举域、可空性、字段必填性或嵌套类型确实不同，才保留不同版本。如果最终只需要一个版本，沿用 Java DTO 对应的原名，不再额外生成基础类型和接口专用类型。

同一个 DTO 确有多个接口版本时，保留通用类型供跨接口的共享组件使用。重新生成时，项目源码和测试中的旧类型引用会在替代目标唯一时自动迁移；存在多个候选时会报告待处理项。原名类型收窄到已证实的枚举后，不属于枚举的手写测试值也会被类型检查发现，需要按业务含义修正。

## 保留不确定性

nlab-api 不把“找到一些值”当成“证明完整值域”。字段语义分为四类：

- `closed`：当前 operation 下所有相关写入都闭合到同一完整枚举域。
- `known`：找到部分关联，但无法证明完整。
- `external`：值来自 RPC、Database 或其他外部边界。
- `unresolved`：调用边、语法、预算或身份存在缺口。

只有 `closed` 可以基于执行图收窄 operation-scoped 响应字段。`closed` 必须同时满足：

- 找到当前 operation 下所有可达写入点。
- 所有分支、setter、builder 和构造参数都已分析。
- 每条写入链都指向同一 enum FQN 和 accessor。
- 枚举全部成员及 wire value 已解析。
- 不存在 RPC、Database、普通变量、字面量、表达式转换或冲突写入。
- 不存在调用边缺失、身份歧义、解析失败、循环、预算耗尽或结果截断。

Javadoc-linked enum 和注释中的显式 coded values 属于声明证据，可以按证据优先级生成字段类型，但不得伪装成执行图 `closed`。`known`、`external`、`unresolved` 保持开放 scalar，并把证据写入诊断。

证据优先级：

1. 完整调用链证明的枚举。
2. 完整 Javadoc-linked Java enum。
3. 注释或注解中的显式 coded values。
4. 原始 scalar。

这条规则宁可少生成枚举，也不制造错误确定性。

## One Contract IR

nlab-api 先构建一份 operation-scoped Contract IR，再从它并行生成：

- Draft OpenAPI 3.1。
- TypeScript 请求、响应和嵌套 DTO。
- 独立 enum 文件。
- 复用项目请求适配器的 API 客户端。

这不是 `Rust -> OpenAPI -> Orval -> TypeScript` 流水线。OpenAPI、API、types 和 enums 是同一份 IR 的兄弟产物，不互相反推。

## 网关路径决定接口入口

后端提供接口目录，nlab-api 先列出目录中的方法，再用 ZGateway 路由筛选。没有对应 HTTP 路径的方法不进入契约和后续枚举分析；查询失败时停止生成。首次生成因此需要可用的网关查询，离线运行只能复用与当前后端提交匹配的既有路由。

其他阶段仍保持明确边界：

- Mock 默认关闭。
- migration 只迁移唯一、可证明的引用。
- `afterGenerate` 只执行项目显式配置的命令。
- 所有产物先校验，再原子提升。

## 为什么使用 Rust

核心流程需要读取 SQLite、解析大量 Java、遍历调用图，并稳定生成大量文件。Rust 提供：

- 单二进制分发。
- 无 Node、Python、Orval 运行时依赖。
- 可控内存、时间预算和错误边界。
- 可重复排序、序列化和文件写入。

AI 不参与运行时契约判断。相同后端 commit、相同配置应得到相同产物。

## 当前边界

nlab-api 当前：

- 读取配置分支的完整契约快照。
- 以配置的 contract roots 限定 Facade。
- 在 RPC、Database 等边界停止。
- 不跨仓库猜测外部服务实现。
- 不根据零散文案推断业务状态。

这些边界让生成结果可验证，也构成后续 Roadmap 的起点。
