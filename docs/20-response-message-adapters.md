# 响应消息 IR 适配器

## 边界

本轮处理四协议**非流式响应中的消息载体**及其内容块，不读取或重写 HTTP 正文。Chat 的调用点是 `choices[].message`，Responses 是 `output[]` 中 `type: "message"` 的项，Messages 是顶层 Message，Gemini 是 `candidates[].content`。选择索引、完成原因、用量和 Gemini 候选元数据仍属于外层响应；Responses 的独立函数调用、推理及服务端工具输出项待 `ResponseOutputItem` 建模后处理。SSE 增量事件不在本轮范围。

## 结构与保留规则

原始载体位于 `protocol/{chat,responses,messages,gemini}/response/message.rs`；与请求对象完全同形的 Responses 输出 Message 和 Gemini Content 复用现有 DTO。响应 IR 位于 `ir/response/message.rs`，与请求 IR 共用 `ir/message.rs` 中的角色、片段和工具调用语义。适配器位于 `adapter/response/<protocol>/{decode,encode}.rs`，输入和输出都是消息序列。

继续使用 `_llmproxy_wire` 记录原协议、形状及未规范化字段。同协议往返恢复 `null`、缺失、扩展字段及不透明块；跨协议只转换文本、普通函数调用及可表达的拒绝，无法安全表达的内容明确报错。IR 正文修改优先于保留字段。不得把整条原消息复制进 metadata。Messages 的顶层 `id/model/usage` 与 Responses 输出消息的 `id/status/type` 没有可由消息 IR 推导的通用值；编码到这些目标协议时需要来源元数据或未来由外层响应模型提供上下文。

## 验收任务

- [x] RM1：四协议响应 Message DTO、响应 IR 和模块出口。
- [x] RM2：四协议非流式响应消息与 IR 的序列级解码和编码。
- [x] RM3：同协议往返、修改后重编码、跨协议文本与函数调用、缺失/null 和不支持内容测试。
- [x] RM4：格式、Clippy、workspace 测试和差异检查。

验收：workspace Clippy 以 `-D warnings` 通过，workspace 测试 121 项通过。

参考：[OpenAI Chat](https://developers.openai.com/api/reference/cli/resources/chat)、[OpenAI Responses](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)、[Claude Messages](https://platform.claude.com/docs/en/api/messages/create)、[Gemini generateContent](https://ai.google.dev/api/generate-content)。
