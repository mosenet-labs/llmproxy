# 跨协议转换：首个闭环与后续任务

## 已确认原则

客户端协议和目标 Provider 协议分别由路由确定。跨协议转换遇到无法等价表达的语义时，默认拒绝并指出字段；不得静默丢弃消息、工具项、缓存控制或生成约束。同协议仍沿用现有协议类型往返路径。JSON 解析与 HTTP 正文缓冲由 Gateway 负责，协议适配器不直接操作 Pingora 分块。

四协议参考：[Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)、[Messages](https://platform.claude.com/docs/en/api/messages/create)、[Gemini generateContent](https://ai.google.dev/api/generate-content)。

## 首个实现范围

本阶段在 `llmproxy-core` 完成四协议两两转换的 JSON 正文编解码，限于非流式、单候选、每条消息只有一段文本、`user`/`assistant` 角色、显式完整历史，以及正常完成的响应。不包含工具、媒体、系统／开发者指令、会话引用、显式缓存设置、采样或结构化输出参数。请求输出上限允许从来源的一个明确字段传递；目标 Messages 必须获得明确上限，不能擅自选默认值。目标模型 ID 来自路由上下文；Gemini 目标模型在 URL 中，不写入 JSON。响应 ID、创建时间和客户端可见模型由调用方提供；不得伪称来源 Provider 生成了这些值。

现有 `ProtocolCodec::encode_request/encode_response` 保持同协议无损往返。新增 `encode_request_for/encode_response_for` 在跨协议时只从 IR 中已规范化的文本消息及用量构造目标协议类型；来源协议类型仅用于检查有无未覆盖语义。对显式报告的缓存读写用量（包括零）、非文本片段、多个候选、非正常完成原因及未覆盖的源字段拒绝转换。Chat 旧版 `max_tokens` 暂不视为等价输出上限。缺失用量保持缺失；若目标协议要求用量则报错，不填零。

网关目前按客户端协议查询路由，`ResolvedModel` 不含独立的 Provider 协议。因此本阶段不改变线上转发路径；将目标协议加入路由数据、HTTP 路径与头改写及失败响应处理属于下一阶段。

## 当前任务与验收

- [x] X1：在 `ProtocolCodec` 定义目标模型／响应外壳输入及跨协议编码入口，保持同协议接口行为。
- [x] X2：实现四种请求目标类型的纯文本编码及来源语义校验；目标 Messages 缺少输出上限时明确报错。
- [x] X3：实现四种非流式响应目标类型的单候选纯文本编码、正常结束原因和用量映射。
- [x] X4：验证 4×4 请求与响应转换矩阵，并覆盖工具项、媒体、缓存、会话引用、多候选和未知字段的拒绝路径。
- [x] X5：受影响 crate 测试 67 项、全工作区测试 155 项通过；全工作区 Clippy、格式和 diff 检查通过。

## 后续待讨论与待办

下列项保持独立，首个闭环完成后逐项讨论语义和验收标准，再实施：

1. 指令：`system`、`developer`、顶层指令和会话中途指令的顺序及优先级。
2. 工具：Responses 独立输入／输出项、四协议工具调用与结果配对、调用 ID 及服务端工具。
3. 媒体：图片、音频、视频、文件的 URL、内联数据、文件引用及上传／下载边界。
4. 生成参数：温度、采样、停止序列、推理配置、输出上限、结构化输出和 Schema 子集。
5. 状态和缓存：Responses 会话引用、Gemini 缓存资源、各协议缓存断点及 TTL。
6. 响应：多候选、有序非消息输出项、拒绝／过滤／未完成原因、错误响应。
7. Usage：缓存读写和模态细分在各协议中的口径、缺失字段与客户端协议的展示方式。
8. 流式：Chat delta、Responses 事件、Messages 内容块事件、Gemini 分块的事件级 IR 与状态机。
9. Gateway：路由记录区分客户端与 Provider 协议；模型 URL／正文位置、鉴权、Content-Length、响应头与失败语义。

历史文档 `21-message-codec-pipeline.md` 描述的是旧版消息节点投影；当前整体同协议入口已改为 `adapter::protocol_codec::ProtocolCodec`。
