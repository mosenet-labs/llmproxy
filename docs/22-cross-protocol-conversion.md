# 跨协议转换：实现范围与后续任务

## 已确认原则

客户端协议和目标 Provider 协议分别由路由确定。跨协议转换优先保持对话可继续：有对应语义时映射，没有对应语义时跳过并返回结构化警告（来源、目标、字段路径、原因），由接入方记日志。不得记录提示词或工具结果正文。缺少目标协议必需字段、工具结果无法配对、无可发送的输入及不支持的流式传输仍报错。同协议仍沿用现有协议类型往返路径。JSON 解析与 HTTP 正文缓冲由 Gateway 负责，协议适配器不直接操作 Pingora 分块。

四协议参考：[Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)、[Messages](https://platform.claude.com/docs/en/api/messages/create)、[Gemini generateContent](https://ai.google.dev/api/generate-content)。

## 首阶段基线（历史记录）

首阶段在 `llmproxy-core` 完成四协议两两转换的 JSON 正文编解码，限于非流式、单候选、每条消息只有一段文本、`user`/`assistant` 角色、显式完整历史，以及正常完成的响应。当时不包含工具、媒体、系统／开发者指令、会话引用、显式缓存设置、采样或结构化输出参数。请求输出上限允许从来源的一个明确字段传递；目标 Messages 必须获得明确上限，不能擅自选默认值。目标模型 ID 来自路由上下文；Gemini 目标模型在 URL 中，不写入 JSON。响应 ID、创建时间和客户端可见模型由调用方提供；不得伪称来源 Provider 生成了这些值。

`ProtocolCodec::encode_request/encode_response` 保持同协议无损往返。首阶段的跨协议入口仅从已规范化的文本消息及用量构造目标协议类型；当时对未覆盖语义直接拒绝。第二阶段已将可丢弃字段改为结构化警告，并加入角色和客户端工具。Chat 旧版 `max_tokens` 暂不视为等价输出上限。缺失用量保持缺失；若目标协议要求用量则报错，不填零。

网关目前按客户端协议查询路由，`ResolvedModel` 不含独立的 Provider 协议。因此本阶段不改变线上转发路径；将目标协议加入路由数据、HTTP 路径与头改写及失败响应处理属于下一阶段。

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

Gateway 尚未区分路由的 Provider 协议；接入跨协议日志须与路由字段、HTTP 路径和头改写一同完成，不在 core 编解码器内直接写日志。

## 后续待讨论与待办

下列项保持独立，首个闭环完成后逐项讨论语义和验收标准，再实施：

1. 服务端内置工具及各协议专属工具参数。
2. 媒体：图片、音频、视频、文件的 URL、内联数据、文件引用及上传／下载边界。
3. 生成参数：温度、采样、停止序列、推理配置、输出上限、结构化输出和 Schema 子集。
4. 状态和缓存：Responses 会话引用、Gemini 缓存资源、各协议缓存断点及 TTL。
5. 响应：多候选、有序非消息输出项、拒绝／过滤／未完成原因、错误响应。
6. Usage：缓存读写和模态细分在各协议中的口径、缺失字段与客户端协议的展示方式。
7. 流式：Chat delta、Responses 事件、Messages 内容块事件、Gemini 分块的事件级 IR 与状态机。
8. Gateway：路由记录区分客户端与 Provider 协议；模型 URL／正文位置、鉴权、Content-Length、响应头、失败语义与转换警告日志。

历史文档 `21-message-codec-pipeline.md` 描述的是旧版消息节点投影；当前整体同协议入口已改为 `adapter::protocol_codec::ProtocolCodec`。
