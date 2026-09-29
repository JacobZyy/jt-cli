# 枚举关联回归样例

这些 Java 文件保存 2026-09-29 调研中的 18 个隔离探针。它们是测试输入，不是业务仓库代码或待集成的前端生成物。断言位于 `src/semantic/coverage_tests.rs`。

普通测试用 Java AST 构造索引边界，随后调用正式契约分析、OpenAPI 和 TypeScript 生成流程：

```sh
cargo test --locked -p nlab-api semantic::coverage_tests
```

安装 CodeGraph 后，同一组断言可改用临时 Git 仓库和全新真实索引。此模式不会读取或改写开发仓库的索引：

```sh
NLAB_API_TEST_REAL_CODEGRAPH=1 cargo test --locked -p nlab-api semantic::coverage_tests
```

覆盖范围：

- `name()`、直接字段、自定义无参取值方法、常量别名，以及 code/name 投影隔离。
- 额外字符串、空字符串、null、初始化值、基本类型默认值和提前返回。额外成员只进入对应字段的枚举版本。
- setter、直接赋值、显式及 Lombok 构造器、可证明的手写及 Lombok Builder。
- 普通方法返回值、按调用点绑定参数、Optional、方法引用、数组和集合元素；生成的容器形状保持不变。
- values 别名、Collectors.toMap、局部 Map 包装的反查。未验证的输入保留开放状态，状态转换不会被当成反查。
- BeanUtils、MapStruct 同名及重命名映射；忽略字段、后续未知覆盖、转换表达式及生命周期回调不产生错误关联。
- 声明为枚举的请求和响应字段，默认名称及可证明的 JsonValue 投影。
- 同名嵌套类、不同实例及对象别名；缺失来源、循环常量、参数改写、动态字符串、辅助属性及自定义序列化的反例。

边界：这是有界静态分析，不执行 Java，不推测缺失依赖的成员。复杂 Builder、转换器、反射和运行时配置不能证明时保留原类型与诊断。全局序列化策略及业务依赖仍需源码或明确配置支持。已知枚举关联与完整闭合值域是不同证据，反查数据库值不会自动证明数据库中不存在其他值。
