# 模型思考开关

## 范围

模型声明思考能力及启用参数，聊天页选择默认／开启／关闭。暂不添加聊天页强度、预算和摘要控制。

## 数据与职责

- Core `thinking`：能力状态、本轮选择、配置校验及 IR 更新。启用参数复用 `ir::request::controls::Reasoning`，不换算强度与 token 预算。
- Store `ModelMapping`：按 Provider／实际模型保存类型化配置。SQLite 与 PostgreSQL 增加可空 TEXT 列，序列化结构体；旧数据为空，解释为未配置。
- Console：模型编辑页配置能力及启用参数；聊天页只提供三态选择。会话保存偏好，开始生成时固定当前轮快照；切换后按新目标重新校验。
- Gateway：按路由实际选中的模型校验并应用配置，随后交给 ProtocolCodec。来源协议不承担思考设置的传输能力。

## 能力与规则

| 状态 | 默认 | 开启 | 关闭 |
| --- | --- | --- | --- |
| 未配置 | 保留客户端配置 | 拒绝 | 拒绝 |
| 不支持 | 保留客户端配置 | 拒绝 | 拒绝 |
| 可开关 | 保留客户端配置 | 应用启用参数 | 应用关闭配置 |
| 始终开启 | 保留客户端配置 | 应用启用参数 | 拒绝 |

配置能力属于管理员声明，不根据模型名称猜测或自动调用 Provider 探测。管理员应确认实际模型接受启用参数，以及是否可以关闭。一个映射支持多个 Provider 协议时，启用参数必须能在全部所选协议中表达。路由页面控件按当前可用目标的能力交集提供选项，Gateway 按真正选中的快照再次校验。

Chat／Responses 通过 effort 开启；Messages 使用 adaptive 或 enabled＋预算；Gemini 使用等级或动态／数字预算。手动 Messages 预算至少 1024，普通请求预算必须小于输出上限；本功能不添加 interleaved thinking 的额外协议支持。关闭清除先前强度和预算，不能被客户端的 high 等级覆盖。

### 开启时请求可见摘要

Gateway 在确定实际目标后设置摘要意图，沿用现有 IR 和 ProtocolCodec；来源协议不必支持摘要开关，不增加页面控件或模型配置列。

| 实际目标 | 开启时的处理 |
| --- | --- |
| Gemini | `reasoning.include = true`，编码为 `thinkingConfig.includeThoughts = true` |
| Responses | 摘要详细程度未指定时设置 `reasoning.summary = auto`；已有详细程度保留 |
| Messages | 使用原生 thinking 输出，不补造摘要控制或签名 |
| Chat | 无标准摘要请求字段；收到的 `reasoning_content` 兼容扩展进入统一 IR |

默认选择完整保留原请求的思考／摘要设置。开启 Gemini 会覆盖原先的 `includeThoughts = false`，以表达页面同时请求可见摘要的选择。关闭不额外修改独立摘要字段。API 返回的是可见思考或摘要，具体内容由模型决定，不能保证每次都有文本。

跨协议流式转回 Chat 时，IR 思考增量写入 `delta.reasoning_content`，与非流式兼容字段一致；记录兼容映射告警，不包含正文。Chat 解码器将此字段还原为 `Head::Reasoning`，页面统一读取 IR，避免重复追加。私有签名沿用既有规则，不进入兼容正文。

## HTTP 与 Pingora

Console 在网关请求中发送 `x-llmproxy-thinking: default|enabled|disabled`。未发送等同 default。该字段仅表达用户意图，不能指定 Provider、模型配置或跳过校验，且在上游发送前删除。

request_filter 解析选择、确定路由、检查能力；独立思考模块在完整请求 IR 上应用配置。默认同协议保持原路径；显式开启／关闭需要缓冲请求并复用子请求准备路径，校验早于上游请求头。响应的同协议 SSE 仍逐块透传，保持原生音频及私有事件；跨协议 SSE 仍按既有 IR 转换。压缩请求若需要修改正文则拒绝，缓冲上限沿用 8 MiB。

显式设置错误返回客户端协议的 422 和安全说明；无效或重复的专用请求头返回 400。模型配置改变不覆盖正在处理的请求快照。

## 验收

1. 旧数据与 SQLite／PostgreSQL 迁移，模型新增、更新和路由快照读回。
2. 四协议同协议与 12 跨方向开启／关闭，强度／预算冲突与输出上限拒绝，Provider 不收到专用头。
3. 同协议 SSE 首段增量、原生事件透传；默认不改变已有请求参数。
4. 模型编辑回显，聊天页能力、模型切换、生成中禁止修改、每轮快照及错误提示。
5. workspace 测试、Clippy、格式检查与浏览器验收。

## 实施结果

T1–T8 已完成。四协议 16 个请求组合分别验证开启和关闭；同协议流式原生事件保持逐块转发。SQLite 与配置的 PostgreSQL 隔离 schema 完成迁移及配置读回。浏览器完成模型配置保存回显、聊天开关、当前轮记录、生成中禁用和模型切换兼容提示。

摘要补充测试覆盖 Gemini 的四种客户端 × 非流式／流式，默认设置保留、Responses 自动摘要与已有详细程度、Chat 兼容增量及畸形／晚到增量拒绝。浏览器验证 Chat→Gemini 的两种响应模式；临时 3210 实例还验证了真实 `gemini-3.5-flash-lite` 的可展开摘要及 894 个推理 token。未修改 3200 服务或真实模型配置。最新工作区测试 339 项通过，11 项既有显式环境／人工用例忽略，Clippy、格式与 diff 检查通过。

使用时先在 Models 编辑模型，声明思考能力与开启参数；Chat 按所选模型提供默认／开启／关闭。旧模型仍为未配置，默认请求保持原行为。声明参数是否被具体 Provider 接受，需要管理员依据实际模型确认。

## 参考

- https://developers.openai.com/api/docs/guides/reasoning
- https://platform.claude.com/docs/en/build-with-claude/thinking-troubleshooting
- https://platform.claude.com/docs/en/build-with-claude/extended-thinking
- https://ai.google.dev/gemini-api/docs/generate-content/thinking
