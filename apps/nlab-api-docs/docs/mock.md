# 语义化 Mock

已有 OpenAPI 就能为每个接口生成一份固定基础样例，不再按枚举状态或场景拆分多份数据。业务资料是可选输入。Mock 不执行按钮后的持久化流转，也不需要后端在线或独立 Mock 服务。

独立 `nlab-api mock` 和 `generate` 中的可选 Mock 阶段复用同一个 Rust 生成器。`jt nlab-api mock` 是兼容入口：未配置 runner 或配置为 `nlab-api` 时转发独立命令，显式 `runner: jt` 时使用内嵌的同一实现；不会自动改写用户选择。

## 直接复用已有契约

```bash
nlab-api mock --project /path/to/frontend --output-root .nlab/generated-mock
```

命令依次读取 `.nlab/openapi.pending.json`、`.nlab/openapi.json`、根目录 `openapi.json`，并复用项目响应包装配置。它不重新提取后端接口，也不修改业务请求 DTO。

`--project` 必填；`--output-root` 默认 `mock`，`--seed` 默认 `42`。`--dry-run` 只校验并报告计划，不写文件，也不把计划数计为已生成数。通过 `nlab-api mock --help` 查看实际参数。

生成阶段的可选 Mock 开关继续使用同一实现：

```json
{
  "mock": {
    "enabled": true,
    "outputRoot": ".nlab/generated-mock",
    "seed": 42,
    "rules": "docs/mock-rules.json"
  }
}
```

已有配置无需增加 `rules`。独立 `mock` 命令也读取配置中的 `mock.rules`；显式 `--rules` 优先。规则文件相对 `--project` 解析，也支持绝对路径。独立命令的输出目录和 seed 保持原有 CLI 默认；需要与自动生成一致时，显式传入这两个参数。

## 添加已确认的业务规则

规则只保存响应差异和资料出处，不复制 DTO 或 Schema。下面的 `DemoFacade#detail`、字段和状态值仅用于展示格式；实际规则必须使用 OpenAPI 的 `x-nlab-operation-key` 和当前契约已有字段、枚举。

```json
{
  "version": 1,
  "locale": "zh_CN",
  "referenceDate": "2026-09-08T00:00:00Z",
  "query": "__mock",
  "operations": {
    "DemoFacade#detail": {
      "coverage": "partial",
      "base": {
        "/orderId": "ORDER-123",
        "/goods/title": "捷安特 ATX 810 山地自行车",
        "/buttons": []
      },
      "generators": {
        "/contact/name": "personName"
      },
      "sources": ["docs/work-order-mock-reference.md"],
      "gaps": ["取消流程尚未确认，见 docs/backend-interface-gaps.md"],
      "assumptions": ["订单标识为固定开发样例"]
    }
  }
}
```

```bash
nlab-api mock --project /path/to/frontend \
  --rules docs/mock-rules.json --output-root .nlab/generated-mock --seed 42
```

- `base` 和 `generators` 的键使用相对响应 data 的 JSON Pointer，例如 `/goods/title`、`/items/0/name`，不包含 `respData`。`~` 和 `/` 在属性名中分别写作 `~0`、`~1`。
- `base` 替换基础对象已有字段；跨接口共享标识由各接口 `base` 固定为相同值。最终响应必须通过契约验证，不能增加契约没有的属性。替换路径不存在时报告该接口失败。
- 旧规则中的 `scenarios`、`defaultScenario`、`coverage` 和 `query` 仍接受原有格式校验，但不再参与响应生成、默认样例选择或 Whistle 分流。需要固定返回值时使用 `base`。
- `sources`、`gaps`、`assumptions` 放在响应外的报告里。资料缺口用文字记录，不能用不存在的字段、枚举或按钮码占位。
- `generators` 支持 `personName`、`email`、`address`、`phone`、`productName`、`description`、`image`、`url`、`dateTime`、`date`、`identifier`、`uuid`、`label`、`statusLabel`、`province`、`city`、`district`、`merchantGroupName`、`roleLabel`、`categoryName`、`brandName`、`modelName`、`count`。它生成字符串；目标字段不接受字符串时校验失败。明确固定值优先于生成器。

没有规则时，工具根据字段名、注释、对象路径、类型和格式选择语义数据。姓名、邮箱使用 Rust `fake 4.4.0` 的 `zh_CN` 数据；省、市、区使用成套的广东省深圳市南山区固定样例，街道门牌为中文开发示意；商品标题、品类、品牌、型号使用同一组捷安特 ATX 810 山地自行车示意数据，图片使用布局占位素材。商户组使用明确的示例名称；角色名称与无法识别的字段会在覆盖报告中标注语义或业务文案未覆盖。具体品牌、型号、品类、金额及时间之间的业务关系应由 `base` 明确表达。

契约 `const`、`example`、`default` 和枚举优先于自动推断；枚举基础样例取首个合法值，不随机决定状态。没有按钮规则时使用空列表并记录未覆盖；若契约要求非空且没有规则覆盖，样例校验失败。可空字段、递归树和数组仍需满足当前契约。复杂 pattern、互斥组合或无法构造的递归结构可能需要显式样例；验证失败会报告原因，不写入不合法的响应。

## 生成结果

每个成功接口只生成一份基础响应，报告档位固定为第一档，不声明多状态业务覆盖。

产物位于 `<output-root>/<appName>/`：

- `<Facade>/<method>.json`：原有默认样例路径。
- `whistle.rules`：接口路径映射到唯一响应文件。
- `coverage.json`：逐接口的生成结果、基础样例档位、输出路径、来源、假设、缺口，以及 seed、语言、参考日期、生成器版本和输入哈希。

生成成功与业务覆盖完整分别报告。一个接口的生成校验失败不会阻止其他接口产出；命令返回退出码 1，报告 `complete-with-errors`，失败接口无新响应文件，Whistle 返回本地 502。业务 TODO 本身不会造成命令失败。自动 `generate` 阶段将生成失败列入警告并保留覆盖报告。

非法规则版本、未知 operation、非法路径或写入保护失败属于输入/写入错误，不能视为业务 TODO。`--dry-run` 不写报告；其返回的报告路径表示计划位置。

## Whistle 路径映射

载入生成的 `whistle.rules` 后，每个接口路径只映射一份响应，不生成状态 Query 规则：

```text
*/api/detail file://</Users/example/project/.nlab/mock/detail.json>
```

文件采用真实绝对路径；尖括号固定文件目标，避免追加剩余请求路径。URL 中的旧 Mock Query 不再切换响应。

## 文件保护与验证

`.nlab/mock-manifest.json` 默认记录生成文件哈希。重新生成时清理清单管理的旧 `.base.json` 和场景文件。覆盖、删除陈旧文件、更新规则前统一检查；人工修改、未受管理的目标文件或路径跨 symlink 时拒绝执行。不要用 `--force` 绕过人工修改保护。

保留有手改的旧 Mock 时，必须同时隔离输出目录和清单。已有清单属于另一输出根时，命令拒绝执行，不会覆盖旧清单：

```bash
nlab-api mock --project /path/to/frontend --rules docs/mock-rules.json \
  --output-root .nlab/semantic-mock --manifest .nlab/semantic-mock-manifest.json
```

`--manifest` 必须是项目内安全相对路径。也可配置 `mock.manifest`，让可选生成阶段沿用相同清单；显式参数优先。新产物只由新清单管理，旧文件、手改规则和旧清单均保留。单独换 `--output-root` 不构成完整隔离。

维护生成器时运行 Rust 全量检查。还可用实际安装的 Whistle parser 校验本轮构建输出，不启动或修改用户正在运行的代理：

```bash
node crates/nlab-api/tests/whistle-mock.mjs \
  /path/to/node_modules/whistle \
  /path/to/generated/whistle.rules \
  POST http://example.test/api/detail
```

### 类型和结构语义

`x-nlab-java-type: Long` 优先于名称推断：即使字段叫 `merchantGroupName`，也生成并验证可解析的 i64 数字字符串，名称与契约冲突记入缺口。计数字符串不会用中文占位。

同时含 `pageNum`、`pageSize`、`total`、`list` 的对象，未被规则固定的分页值会按生成列表计算；显式合法分页值保留，矛盾值报告失败。没有业务规则的 `statusTabs` 只生成一项并说明映射未覆盖，不随机拼出生产状态。商品开发标识不代表真实品类库映射；明确契约样例、枚举和业务规则仍优先。
