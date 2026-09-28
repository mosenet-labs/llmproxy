# 模型映射与 Models 控制台

## 已确认的行为

- 控制台的两个管理入口为 `/ui/providers`（Providers）和 `/ui/models`（Models）；`/ui` 跳转到 Providers。
- 新建模型时先选一个**已启用**的 Provider，再通过其已保存的模型探测接口读取候选模型 ID。候选项可搜索、多选；未列出的模型可手动输入 ID，模型列表接口失败时也可手动添加。探测协议与 Provider 的 Messages 鉴权设置共同决定 `/models` 的鉴权头，不决定模型可用的推理协议。
- 默认别名为 `Provider名称/上游模型ID`，保存前可编辑；此后 Provider 改名不自动改别名。别名全局唯一，一个模型记录对应一个 Provider 和一个上游模型 ID。
- 从该 Provider 已配置的 Chat、Responses、Messages 接口中选择至少一个协议。一个模型记录可以同时支持多个协议。保存时校验 Provider 状态、模型 ID 和协议配置，不要求模型 ID 出现在 `/models` 结果中；可在保存后使用推理探测确认可用性。
- `(请求协议, 请求体中的别名)` 唯一确定上游 Provider、路径和模型 ID。网关把顶层 JSON `model` 改成上游模型 ID 后转发；响应体和 SSE 原样透传。
- 不再存在按协议的“当前 Provider”兜底。缺少或无效的 `model` 返回客户端错误，别名或协议未配置返回模型未配置，映射到已停用 Provider 返回服务不可用。零映射是允许的启动状态。
- 停用 Provider 保留映射，但请求不可用；删除仍被映射使用的 Provider、移除映射正在使用的协议均被拒绝，须先调整或删除映射。
- 模型列表探测结果只用于供选择的候选项；探测本身不发布路由，也不保证模型可用。路由概览显示协议入口和已配置模型数。

## 模型可用性探测

`/models` 只用于发现候选 ID，不能证明模型仍接受推理请求。Models 编辑弹窗可对已保存的模型逐协议执行一次短推理请求：输入为 `你好`，输出上限默认 1 token，可临时调整至 1–1024。Chat 先用兼容上游常见的 `max_tokens`，仅当上游明确拒绝该字段时改用 `max_completion_tokens` 重试；Responses 用 `max_output_tokens`，Messages 用 `max_tokens`。请求使用该 Provider 已保存的协议路径和凭据；可用性结果按 Provider、上游模型 ID、协议区分，不能由一个协议的结果推断另一个协议。

探测请求显式尝试关闭思考：Chat 发送 `thinking.type=disabled`，仅在上游明确不识别该字段时改用 `reasoning_effort=none`；Responses 发送 `reasoning.effort=none`；Messages 发送 `thinking.type=disabled`。不移除思考控制参数后重试。对于已知强制思考的 `glm-5.3`/`glm-5.3-flash`，直接发送 `reasoning_effort=low`；若其他模型明确拒绝关闭思考，则按协议改用最低的 `low` 强度重试，并在结果中提示仍会消耗思考 token。若上游接受关闭参数却仍返回可识别的思考内容或非零思考 token，探测标记为无法判定。兼容上游可能忽略参数且不提供思考用量，因此“请求关闭思考”不能保证所有服务实际关闭。

公共 `llmproxy-probe::ModelProber::probe_model` 只发送请求并返回结构化结果，不读写数据库或 UI。有效的推理响应标记为可用；上游明确返回 `model_not_found` 标记为不可用；鉴权失败、限流、超时、参数不兼容、普通 404 和无效响应均标记为无法判定。输出上限包含部分模型的推理 token，因此成功响应不保证有可见文本。页面显示本次结果；遥测记录状态、耗时和 token 用量，不保存探测输入或输出正文，也不自动删除模型或修改路由。

后续若加入定时探测和历史日志，仍调用同一公共方法；历史记录须单独设计保留期及敏感内容处理。

## 数据与热更新

新增 `model_mappings` 表：`id`、唯一 `alias`、`provider_id`、`upstream_model_id`、三个协议开关、`version`、`updated_at`。PostgreSQL 和 SQLite 各追加一条迁移，不修改已有迁移历史；不要求两种数据库互迁。旧 `route_bindings` 保留在历史 schema 中，但不参与请求路由。

Toasty 负责 CRUD 和版本冲突校验。Provider 配置、映射、密钥由数据库加载为不可变快照，每秒原子替换。请求开始时固定一次映射和 Provider 配置，避免长时间 SSE 中途切换。数据暂时不可读时保留上一版快照并记录遥测。

## Pingora 阶段

| 阶段 | 工作 |
| --- | --- |
| `request_filter` | 确定明确协议；从顶层 JSON `model` 读取别名、校验映射与 Provider 状态，并在选择上游前固定请求配置。通过 Pingora 的请求重试缓冲保存已读前缀。 |
| `upstream_peer` | 根据固定配置创建 `HttpPeer`，沿用现有 DNS、连接和 TLS 遥测。 |
| `upstream_request_filter` | 修改上游路径、Host、凭据和请求体长度。 |
| `request_body_filter` | 在第一次正文回放时将别名改成上游模型 ID；余下正文继续流式转发。 |
| 响应阶段 | 保持现有错误、SSE 和访问遥测行为。 |

Pingora 0.9 的请求体过滤器发生在上游选择之后，故必须在 `request_filter` 读取到 `model`。重试缓冲上限为 64 KiB：`model` 字段必须完整出现在此前缀内；超过后拒绝请求，后续正文仍可流式转发。只接受未压缩 JSON 请求体。当前服务监听明文 HTTP/1.1，已用不同别名长度和大正文测试验证重放与长度处理；下游 TLS/HTTP/2 监听和 H2 端到端验证列入[M6](02-tasks.md#m模型映射)。

## 页面

Models 列表显示别名、上游模型、Provider、协议和状态，提供局部搜索、创建、编辑、删除。新建和编辑使用 `topcoat-ant-design` 的原生 Dialog、表单字段、标签、确认气泡与通知。状态和异步提交使用 Topcoat Signal、Shard、Procedure；新建弹窗使用可搜索多选候选框，并提供“手动添加模型 ID”入口。手动输入的 ID 与候选模型进入同一批待导入列表，默认别名为 `Provider名称/模型ID`，仍可修改；编辑弹窗也允许直接修改模型 ID。列表不显示凭据。启用 Provider 后即可被候选列表选择；停用后现有模型显示不可用。

模型参考价格从现有简单输入/输出字段扩展为按 Provider + 上游模型共享的版本化规则，单独见[模型参考价格规则](16-model-pricing.md)与[B：实施任务](02-tasks.md#b模型参考价格规则)。

## 验收

1. 双数据库迁移、模型 CRUD、别名唯一、版本冲突、协议至少一项及 Provider 约束。
2. `/models` 候选搜索、多选与手动添加；接口不可用时仍可保存有效的手动模型 ID；新建/编辑后局部更新和一次性通知。
3. Chat、Responses、Messages 对同一别名的正确路径、凭据、模型 ID 改写；未映射、禁用、缺少 `model`、不同长度及流式响应。
4. 删除/协议移除的引用保护，快照热更新与已有 SSE 稳定。

## 实施与验证记录

M1–M5 已完成。SQLite 与 PostgreSQL 的追加迁移、模型 CRUD、别名唯一、版本冲突及引用保护均由自动测试覆盖。三个明确入口的别名匹配、请求体改写、缺失/未知模型、大正文、错误与 SSE，以及数据库热更新期间正在进行的 SSE 均通过本地模拟上游验证。浏览器在隔离 SQLite 环境验证了 Topcoat 弹窗、120 个候选模型、多协议保存、默认别名、一次性通知和局部搜索。完整 workspace 测试使用独立 PostgreSQL schema 与临时 SQLite 文件，未触碰开发数据库现有表。H2 验证见 M6。
