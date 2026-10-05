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
8. Gateway：非流式 HTTP 链路已接入；非流式正文继续限制为 8 MiB，响应发头前的失败状态已回写；后续处理流式事件转换。

历史文档 `21-message-codec-pipeline.md` 描述的是旧版消息节点投影；当前整体同协议入口已改为 `adapter::protocol_codec::ProtocolCodec`。


## 第六阶段：完整非流式能力（优先于流式）

2026-10-03 确认：先补齐全部非流式能力，再启动流式。这里的“完整”指对已声明字段给出明确的映射或不兼容处理；不同 Provider 的私有资源、服务端执行环境与加密数据不能伪造等价物。跨协议仍只经类型化协议载体和 IR，JSON 解析／序列化留在 HTTP 边界。

实施顺序及验收：

- [x] F1：结束状态、拒绝、空输出和候选边界；支持截断／过滤，不能伪装为正常结束。失败和取消进入 IR，后台跨协议拒绝，控制台明确报错。
- [ ] F2：cache / usage 按目标能力映射；校验计数一致性，保留缺失与零的区别，仅对无法表达的细分发出警告。
- [ ] F3：生成参数、工具选择、并行调用、结构化输出及推理配置；校验目标范围，专属字段显式诊断。
- [ ] F4：图片、音频、视频、文件和工具结果内容；保留顺序与可移植数据，对 Provider 文件 ID 等资源引用明确报错。
- [x] F5：缓存配置、断点、会话引用、服务端工具和专有输出；可映射时映射，依赖远端状态且无法转换时明确拒绝。
- [x] F6：非流式错误及 HTTP 边界；完善转换失败返回、正文与模型上下文处理。
- [x] F7：页面可选择非流式，使用协议 struct 发起请求及读取完整响应，展示实际 usage/cache；四客户端文本浏览器验收通过，工具／媒体展示另列待办。
- [ ] F8：四协议方向矩阵、同协议回归、Gateway 与页面验证，更新支持表后才开始流式。

流式后续：事件 IR、四协议事件编解码状态机、SSE 分帧与取消、usage/cache 累积、Gateway 逐事件转换以及页面验证。禁止通过先收完整响应再伪装成流来代替事件级实现。

### 本轮落地与剩余验收

| 能力 | 已实现 | 尚待完成 |
| --- | --- | --- |
| 状态与候选 | 截断、过滤、拒绝、Gemini 生成前过滤；Chat/Gemini 多候选；单候选目标选择最小序号并警告 | Responses 失败／后台状态与统一错误 IR 的衔接 |
| 用量 | 输入、输出、总量、缓存读写、推理及目标支持的模态细分；一致性校验；缺失不伪造零 | 缓存／工具输入的模态细分完整归一化 |
| 请求控制 | 采样参数、具名／允许列表工具选择、并行控制、输出 Schema、推理等级／预算；同协议编辑保护 | 专属生成配置的逐字段验收与模型能力约束 |
| 输入媒体 | 图片／PDF 的四协议转换；Chat↔Gemini 支持的内联音频；Gemini 视频载体；私有文件引用拒绝 | 文本文档、媒体附加配置和输出音视频的完整映射 |
| 工具结果 | Messages／Responses／Gemini 的文本和媒体；错误标志；服务端代码／搜索输出进入独立 IR，可见内容转换为文本；不伪造客户端调用 | 特殊附件的跨 Provider 物化和原生展示不在本次转换范围，明确告警或拒绝 |
| 缓存与状态 | 指令、消息片段、函数声明的断点进入 IR；保留可表达的边界／TTL；验证位置、数量、TTL 顺序；会话／缓存／容器私有引用拒绝 | 资源物化需要调用来源服务，当前不自动下载或上传 |
| Gateway | 父请求缓冲跨协议转换结果后发头；错误 JSON、提前断开、读取超时分别返回 502／504；原始请求前缀只回放一次；遥测完成统计去重 | 超过 8 MiB 明确拒绝；客户端断开时的主动中止与真实 Provider 长请求仍需专项验收 |
| 页面 | 默认保持已有同协议流式，可通过开关选择非流式；类型化请求／完整响应；逐轮 usage/cache，统计不进入下一轮历史 | 浏览器端完整验收与多模态／工具输出展示 |

### 缓存断点、专属工具与响应头补齐（2026-10-03）

代码组织：

- `ir/cache.rs`：`Breakpoint` 与 `CacheLocation`；断点关联指令、消息片段或函数位置。
- `ir/request/native.rs`：服务端搜索／代码执行声明，与客户端函数分开。
- `ir/server_output.rs`：服务端输出的可见正文、语言和状态；原始执行记录仅用于同协议往返。
- `adapter/protocol_codec/projection/{cache,native}.rs`：从来源类型提取公共语义、记录无法转换的字段；只解释开放的局部叶子。
- `adapter/protocol_codec/cross/{cache,native,server_output}.rs`：目标能力校验和构造；跨协议不读取来源副本。
- `gateway/src/proxy/{mod,buffered}.rs`：Pingora 回调与延迟提交响应分别负责；回调保持简洁。

| 内容 | 转换规则 |
| --- | --- |
| Messages 内容／系统／函数断点 | 解码为明确位置与 TTL；目标 Messages 保持边界，支持 `5m`、`1h`，最多四个显式断点，长 TTL 在短 TTL 前；自动缓存可与末尾同 TTL 断点共用位置 |
| Chat／Responses 内容断点 | 映射已有协议类型的 `prompt_cache_breakpoint`；不复制 Messages TTL；函数声明无对应断点字段时告警；Responses 指令有断点时整组转 system 输入保持顺序 |
| Gemini 内容断点 | 生成接口没有等价位置，告警丢弃缓存配置，保留实际输入内容；缓存资源 ID 不能作为可移植上下文 |
| Web search | 四协议构造各自声明；Responses／Messages 保持域名限制；Chat 与 Gemini Developer API 无等价域名过滤时拒绝，不能扩大搜索范围；搜索引擎和排序差异告警 |
| Code execution | Responses 自动新容器、Messages `code_execution_20250825`、Gemini `codeExecution`；Chat 没有此能力，告警丢弃。运行环境差异告警，不复制容器／文件 ID |
| 工具选择约束 | 必须调用但目标无法保证时拒绝；禁用工具或仅允许客户端函数时，不额外启用服务端工具；不把服务端输出生成客户端 `tool_calls` |
| 服务端可见输出 | Gemini 代码／结果、Responses 执行日志、Messages 执行结果／搜索标题与 URL 经 IR 转成有损文本，记录警告；执行 ID、加密结果及专属附件不复制 |
| 其他专属工具 | 计算机操作、地图、MCP、自定义非函数工具等无等价公共能力时按既定策略告警丢弃；若显式工具选择依赖被丢弃声明则拒绝。文件库、输入项引用、容器文件上传等承载私有上下文时拒绝 |
| 同协议往返 | 原始专属字段保持不变；修改断点或内置声明后需移除来源副本进行规范化重建，禁止静默忽略编辑 |

HTTP 流程：父请求完成路由、保存已读模型前缀并缓冲剩余原始请求体；通过 `SubrequestSpawner::create_subrequest` 创建子请求，移动已选 Provider 快照与转换上下文；子请求沿用现有鉴权、连接、请求／响应转换回调；父请求收完 `HttpTask`、核对结束标记与失败通道后才提交真实响应头和正文，并重算 Content-Length。正文发送完仍保持请求通道发送端存活，避免被框架误判为客户端断开。子请求归还上下文，由父请求记录一次完成统计。没有游离后台任务；父 future 被取消时，子请求 future 一起销毁。

验收覆盖：三种缓存位置、TTL 丢弃与非法顺序、断点越界／重复／数量、自动缓存共用断点、四协议搜索矩阵、搜索限制、私有执行状态、代码工具有无能力、服务端输出不变成客户端调用、同协议原文保留、未知工具告警；HTTP 覆盖转换成功、429、上游 200 后正文错误／截断／超时、100 KB 请求前缀回放及统计去重。另修复 Responses 字符串 `input` 未进入消息 IR 的问题。

参考文档：

- [Messages 缓存](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [Responses 搜索](https://developers.openai.com/api/docs/guides/tools-web-search)、[代码执行](https://developers.openai.com/api/docs/guides/tools-code-interpreter)
- [Messages 搜索](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)、[代码执行](https://platform.claude.com/docs/en/agents-and-tools/tool-use/code-execution-tool)
- [Gemini 搜索](https://ai.google.dev/gemini-api/docs/google-search)、[代码执行](https://ai.google.dev/gemini-api/docs/code-execution)、[GoogleSearch SDK 字段边界](https://googleapis.github.io/js-genai/release_docs/interfaces/types.GoogleSearch.html)

上述剩余项仍属于非流式阶段；未全部验收前不启动跨协议流式实现。

上一轮验证：`cargo test --workspace` 共 204 项通过；`cargo clippy --workspace --all-targets -- -D warnings`、格式检查与 `git diff --check` 通过。验证包含模拟 Provider 的 HTTP 集成测试；尚未进行真实 Provider 或浏览器端完整验收。

本次补齐验证：`cargo test --workspace` 共 215 项通过（21 个测试套件）；`cargo clippy --workspace --all-targets -- -D warnings`、格式检查与 `git diff --check` 通过。包含模拟 Provider 的真实 HTTP 测试；未进行真实 Provider 与浏览器完整验收，其他非流式待验收项保持开放。

### 非流式矩阵与状态补齐（2026-10-05）

字段覆盖、转换策略、验收入口和仍开放事项见 [非流式验收记录](23-nonstream-acceptance.md)。新增缓存/工具输入模态公共计数，修复缺失候选序号，补齐生成失败 IR 和取消状态；文本文档可显式降为文本，输出音频缺少格式上下文时明确拒绝。Gateway 补齐延迟发头期间的客户端取消监视和已知长度超限拒绝。

模拟 HTTP 的 12 个跨协议方向及 4 个同协议方向已验证文本、函数声明与具名选择、调用/结果双回合、429、总量和缓存读取。真实 Provider 的 16 个文本和自动工具双回合通过；DeepSeek 实际报告缓存读取 128 tokens。Gemini 的 3 个跨协议工具方向通过进程内短期签名状态修复并真实复验，详细隔离、TTL、容量与多实例限制见文档 23。OpenObserve 已使用原配置的有效写入 token 上报，查询认证单独注入，三类遥测与用量读回通过；多模态、缓存写入/TTL和浏览器验收仍开放，因此 F8 与跨协议流式保持未完成。

### 工具状态持久化与专项验收补齐（2026-10-05）

- [x] P1：复用既有 PostgreSQL／SQLite 和主密钥，加密保存原始调用片段；状态闲置期 24 小时，容量及整轮保存使用事务。
- [x] P2：目标请求构造后异步恢复；父请求发头前保存响应状态；存储故障、超时、过期和作用域错误明确拒绝。
- [x] P3：三种客户端跨实例、重启、重复历史、过期及存储故障；SQLite／PG 跨连接、并发保存和主密钥验收。
- [ ] P4：结构化输出、图片／PDF、音视频的真实专项联调与字段边界验收；当前 Gemini 输入专项及四协议有效样本 HTTP 矩阵已完成，其他原厂能力与媒体输出仍待验收。
- [ ] P5：缓存写入／TTL、页面非流式显示、停止与统计的完整验收；模拟缓存写入／TTL 投影、OpenObserve 读回和四客户端文本浏览器验收已完成，真实模型缓存创建及工具／多模态展示仍待验收。
- [ ] P6：关闭非流式验收项后，实施事件 IR 与跨协议流式。
