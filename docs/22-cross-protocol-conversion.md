# 跨协议转换：实现范围与后续任务

## 已确认原则

客户端协议和目标 Provider 协议分别由路由确定。跨协议转换优先保持对话可继续：有对应语义时映射，没有对应语义时跳过并返回结构化警告（来源、目标、字段路径、原因），由接入方记日志。不得记录提示词或工具结果正文。缺少目标协议必需字段、工具结果无法配对、无可发送的输入及不支持的流式传输仍报错。同协议仍沿用现有协议类型往返路径。JSON 解析与 HTTP 正文缓冲由 Gateway 负责，协议适配器不直接操作 Pingora 分块。

四协议参考：[Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)、[Messages](https://platform.claude.com/docs/en/api/messages/create)、[Gemini generateContent](https://ai.google.dev/api/generate-content)。

## 首阶段基线（历史记录）

首阶段在 `llmproxy-core` 完成四协议两两转换的 JSON 正文编解码，限于非流式、单候选、每条消息只有一段文本、`user`/`assistant` 角色、显式完整历史，以及正常完成的响应。当时不包含工具、媒体、系统／开发者指令、会话引用、显式缓存设置、采样或结构化输出参数。请求输出上限允许从来源的一个明确字段传递；目标 Messages 必须获得明确上限，不能擅自选默认值。目标模型 ID 来自路由上下文；Gemini 目标模型在 URL 中，不写入 JSON。响应 ID、创建时间和客户端可见模型由调用方提供；不得伪称来源 Provider 生成了这些值。

`ProtocolCodec::encode_request/encode_response` 保持同协议无损往返。首阶段的跨协议入口仅从已规范化的文本消息及用量构造目标协议类型；当时对未覆盖语义直接拒绝。第二阶段已将可丢弃字段改为结构化警告，并加入角色和客户端工具。Chat 旧版 `max_tokens` 暂不视为等价输出上限。缺失用量保持缺失；若目标协议要求用量则报错，不填零。

历史基线：当时网关只按客户端协议查询路由，`ResolvedModel` 没有独立的 Provider 协议。第三阶段已为显式模型路由增加 Provider 协议；未创建显式路由的模型标识仍按原协议直连。

## 首阶段任务与验收（已完成）

- [x] X1：在 `ProtocolCodec` 定义目标模型／响应外壳输入及跨协议编码入口，保持同协议接口行为。
- [x] X2：实现四种请求目标类型的纯文本编码及来源语义校验；目标 Messages 缺少输出上限时明确报错。
- [x] X3：实现四种非流式响应目标类型的单候选纯文本编码、正常结束原因和用量映射。
- [x] X4：验证 4×4 请求与响应转换矩阵，并覆盖工具项、媒体、缓存、会话引用、多候选和未知字段的拒绝路径。
- [x] X5：受影响 crate 测试 67 项、全工作区测试 155 项通过；全工作区 Clippy、格式和 diff 检查通过。

## 第二阶段：角色和客户端工具

IR 将顶层指令与有序消息分开；Responses 独立的 `function_call`／`function_call_output` 进入有序输入或输出项。调用 ID 是配对依据；Responses 输出项 ID 与 `call_id` 分开保存。只有能还原为目标协议合法工具调用与结果时才转换，避免制造未配对结果。服务端内置工具暂不伪装为客户端函数工具。

| 来源语义 | Chat | Responses | Messages | Gemini |
| --- | --- | --- | --- | --- |
| 顶层指令／开头 `system` | `system` | `instructions` 或 `system` 输入项 | 顶层 `system` | `systemInstruction` |
| 开头 `developer` | `developer` | `developer` 输入项 | 顶层 `system`，警告优先级合并 | `systemInstruction`，警告优先级合并 |
| 中途 `system`／`developer` | 原顺序保留 | 原顺序保留 | 降为原位置 `user`，警告优先级降低 | 降为原位置 `user`，警告优先级降低 |
| `user`／`assistant` | 原角色 | 原角色 | 原角色 | `user`／`model` |
| 客户端工具调用／结果 | `tool_calls`／`tool` | 独立 `function_call`／`function_call_output` | `tool_use`／`tool_result` | `functionCall`／`functionResponse` |
| 未指定角色 | 丢弃并警告 | 丢弃并警告 | 丢弃并警告 | 保持未指定 |

Messages 的中途 `system` 只在部分模型受支持；路由尚无模型能力标记，因此本阶段采取降级策略。`instructions` 转 Chat 为开头 `system` 时只保证近似语义，返回警告。开头高优先级指令的相对顺序应保持；无法保持优先级时返回警告。

### 实施任务与验收

- [x] Y1：IR 增加顶层指令和有序工具输入／输出项；四协议解码后能观察到角色、顺序、调用 ID，原协议往返仍无损。顶层指令及独立项目前只读，编辑后同协议回写会明确报错；跨协议编码会拒绝未被有序项引用的消息。
- [x] Y2：跨协议编码返回正文及结构化警告；同协议警告为空；警告只含协议、路径和原因，不含正文。
- [x] Y3：实现上述角色和顶层指令映射；验证开头与中途指令、无角色及优先级降级。
- [x] Y4：实现四协议客户端函数声明、调用及结果的请求映射和响应工具调用映射；验证顺序、并行调用／结果成组、ID、名称与参数，无法配对时报错。Gemini 的通用 JSON Schema 使用 `parametersJsonSchema`，原生 Schema 转其他协议时规范化类型枚举。
- [x] Y5：core 测试 82 项、全工作区测试 170 项通过；全工作区 Clippy、格式与 diff 检查通过。

Gateway 在转换过程中记录结构化警告；日志只含来源、目标、字段路径和原因。

## 第三阶段：Gateway 非流式转发

显式模型路由分别保存客户端协议和 Provider 协议；候选模型按 Provider 协议校验。旧路由迁移时将 Provider 协议填为原路由协议。路由选中后，Gateway 固定 Provider 协议、上游模型和客户端模型标识。请求 JSON 在 Pingora 正文回调中缓冲至结束，按 `客户端协议 → IR → Provider 协议` 转换；响应成功正文反向转换。HTTP 请求按目标协议设置路径、模型位置和鉴权；跨协议请求移除旧 `Content-Length`，由 Pingora 为 HTTP/1.1 生成分块传输。响应转换前移除 Provider 的正文长度，并由 Pingora 重新定帧。

上游非成功状态保留 HTTP 状态，正文替换为客户端协议的通用错误外壳，不透传 Provider 原始错误内容。转换警告以结构化日志记录，不记录对话正文。跨协议仅支持未压缩的非流式 JSON，正文上限 8 MiB；不支持的请求在正文发送前报错。响应头已发送后若正文转换失败，连接会终止，此时不能再改写 HTTP 状态。后续可评估在下游发头前完整缓冲上游响应，以消除这一限制。

- [x] Z1：路由存储、迁移和控制台区分客户端与 Provider 协议，原有路由同协议行为不变。
- [x] Z2：Gateway 按目标协议改写路径、鉴权和请求正文；按客户端协议改写成功响应及失败外壳。
- [x] Z3：验证实际 Chat→Messages、Chat→Gemini 请求及反向响应、用量、HTTP 分块定帧和 Provider 错误；全工作区 174 项测试通过，Clippy 无警告。
- [ ] Z4：事件级流式转换；需先完成流式 IR 和各协议事件状态机。

## 第四阶段：最小整体 IR 与独立目标编码

请求 IR 增加模型、客户端函数声明和通用生成参数；响应 IR 增加模型、ID、创建时间、整体状态、候选边界和结束原因。原有消息、指令、有序项、缓存和用量继续独立保存。来源协议类型变为可选的同协议往返副本，`without_source()` 可将其移除；也可用 `Request::new` / `Response::new` 手工构造 IR。来源协议标识只为诊断提供上下文，跨协议目标正文由 IR 通用字段构造。

`projection/` 在解码阶段提取字段并记录未映射语义；`cross/` 从 IR 构造目标正文，既不读取来源 DTO，也不在来源 JSON 上回填。`roundtrip.rs` 负责保留来源副本时的同协议通用字段编辑。新增字段若无法表达或无法保留专有语义则报错；顶层指令、有序项以及响应状态、候选边界的同协议结构编辑仍暂不支持。

| IR 生成参数 | Chat | Responses | Messages | Gemini |
| --- | --- | --- | --- | --- |
| `max_output_tokens` | `max_completion_tokens` | `max_output_tokens` | `max_tokens`，必需 | `generationConfig.maxOutputTokens` |
| `temperature` | `temperature` | `temperature` | `temperature` | `generationConfig.temperature` |
| `top_p` | `top_p` | `top_p` | `top_p` | `generationConfig.topP` |
| `stop_sequences` | `stop`，最多 4 项 | 丢弃并警告 | `stop_sequences` | `generationConfig.stopSequences`，最多 5 项 |
| `candidate_count` | `n` | 固定单候选，不生成字段 | 固定单候选，不生成字段 | `generationConfig.candidateCount` |
| `stream` | `stream` | `stream` | `stream` | URL 操作名，由 Gateway 判断 |

参数未指定时不生成目标字段。温度按目标协议范围检查：Messages 为 0～1，其余为 0～2；top-p 为 0～1，具体模型的能力限制仍由 Provider 校验。跨协议候选数目前仅允许 1，流式请求仍明确拒绝。Gemini 请求的模型和流式模式不从 JSON 同名扩展字段推断。

响应保留候选与有序输出项之间的索引关系，防止把多个候选展平成一个回复。目前目标编码仍只接受单候选、正常完成或工具调用；长度截断、过滤等原因可进入 IR，但暂不生成跨协议响应。创建时间缺失时不伪造，调用方通过 `ResponseTarget` 提供目标响应外壳。缓存与用量的原始统计继续留在 IR；跨协议正文当前仅映射基础输入、输出和总计，细分项丢弃并警告。

同协议且仍有来源副本时，`encode_request_for` / `encode_response_for` 保留原往返行为，不套用目标外壳覆盖。跨协议或移除副本后使用目标编码。直接 `encode_request` / `encode_response` 从 IR 取得模型和响应外壳，只返回正文；Gateway 使用带 `_for` 的入口取得警告并记录日志。

- [x] N1：请求加入模型、函数声明、输出上限、温度、top-p、停止序列、候选数及流式标记；响应加入模型、ID、时间、状态和候选边界。
- [x] N2：来源字段检查移到解码阶段；以不含正文的诊断记录尚未映射的语义，目标编码不再读取来源类型。
- [x] N3：保留同协议往返路径；新增通用字段的编辑必须生效或明确报错，不能静默忽略。
- [x] N4：验证无来源类型的 4×4 编码、IR 编辑、诊断继承、同协议往返及 Gateway 回归；core 93 项、全工作区 185 项测试通过。

## 第五阶段：协议 struct → IR → 目标协议 struct

`ProtocolCodec` 的输入和输出改为 `protocol::Request` / `protocol::Response`。这两个枚举只包裹四协议的具体结构体，`Conversion<T>` 保存目标类型及转换警告。编解码器不接收或返回整包 `serde_json::Value`，也不负责 JSON 字节的解析、序列化。

```text
来源 protocol::Request  → ir::request::Request  → 目标 protocol::Request
来源 protocol::Response → ir::response::Response → 目标 protocol::Response
```

模块职责如下：

| 模块 | 职责 |
| --- | --- |
| `protocol/request.rs`、`protocol/response.rs` | 四协议结构体的统一载体 |
| `adapter/protocol_codec/projection/` | 从类型字段提取通用参数、候选和转换诊断 |
| `adapter/protocol_codec/cross/` | 直接构造目标请求、响应和工具项 |
| `adapter/protocol_codec/roundtrip.rs` | 同协议直接回写结构体，只更新实际编辑字段 |
| `adapter/request/`、`adapter/response/` | 直接匹配消息／内容块类型，投影和构造消息字段 |
| `gateway/transform/codec.rs` | HTTP 边界按路由协议反序列化、序列化 |

函数声明以及 Responses 独立函数调用、结果补充了类型声明。已知工具项不能因字段无效而静默退回未知项；兼容 Provider 未提供调用 ID 的情况，仍沿用显式补号及配对警告规则。JSON 测试夹具的转换辅助仅在测试模块存在，不构成生产接口。

`Value` 仍用于工具的动态参数／结果、JSON Schema、未知扩展、不透明内容块和 IR 元数据中的剩余叶子字段。这些内容不作为整条消息或完整请求／响应的中转载体。专属子对象的进一步类型细化、跨协议语义支持范围与本阶段的链路职责重构分别跟踪；本阶段不意味着所有跨协议语义已实现。

- [x] S1：公开协议请求／响应载体，编解码 trait 直接接收和返回类型。
- [x] S2：请求、响应投影和目标构造去掉整包 JSON 中转，同协议编辑直接回写类型字段。
- [x] S3：替换下层消息适配器对整条消息的 JSON 往返，补充函数声明及独立工具项类型。
- [x] S4：Gateway 承担 JSON 边界；同协议无编辑时保留原始字节。
- [x] S5：验证类型化 4×4 链路、字段编辑、工具配对、同协议保留及 Gateway 回归；core 98 项、全工作区 189 项测试通过，含 null 编辑后无重复字段的回归；全工作区 Clippy、格式与 diff 检查通过。已删除不再使用的 JsonDocument 及其专用测试。

## 后续待讨论与待办

下列项保持独立，首个闭环完成后逐项讨论语义和验收标准，再实施：

1. 服务端内置工具及各协议专属工具参数。
2. 媒体：图片、音频、视频、文件的 URL、内联数据、文件引用及上传／下载边界。
3. 生成参数：补充尚未归一化的采样参数、推理配置、结构化输出、模型能力限制和 Schema 子集。
4. 状态和缓存：Responses 会话引用、Gemini 缓存资源、各协议缓存断点及 TTL。
5. 响应：多候选、有序非消息输出项、拒绝／过滤／未完成原因、错误响应。
6. Usage：缓存读写和模态细分在各协议中的口径、缺失字段与客户端协议的展示方式。
7. 流式：Chat delta、Responses 事件、Messages 内容块事件、Gemini 分块的事件级 IR 与状态机。
8. Gateway：非流式 HTTP 链路已接入；后续处理大于 8 MiB 的正文、响应发头前的失败状态回写，以及流式事件转换。

历史文档 `21-message-codec-pipeline.md` 描述的是旧版消息节点投影；当前整体同协议入口已改为 `adapter::protocol_codec::ProtocolCodec`。
