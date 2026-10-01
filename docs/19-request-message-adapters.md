# 请求消息 IR 适配器

## 范围与假设

客户端协议和上游协议由路由确定。本阶段仅处理四种协议请求中的消息序列，不接入 Pingora，也不转换请求顶层字段或响应。Responses 仅覆盖现有 `Message` DTO；独立的 `function_call`、`function_call_output` 等输入项须待外层 RequestItem 建模后接入。

首批跨协议语义为文本、普通函数调用及结果。图片、音视频、文件、思考块、自定义或服务端工具等内容可在原协议往返；目标协议不同且不能安全表达时返回明确错误。函数参数如果是 JSON 文本，解码时必须验证为 JSON 对象，避免把无效文本伪装成结构化参数。

## 模块和数据流

`protocol/*/request/message.rs` 继续只定义原始数据；`ir/request/message.rs` 定义中间语义；`adapter/request/` 各协议模块按**消息序列**解码、编码。`wire.rs` 只处理各协议共用的保留字段和内容块操作；四个协议均在各自目录下以 `decode.rs`、`encode.rs` 分开实现。调用方先以客户端协议解码到 IR，处理 IR，再按 Provider 协议编码。适配器不读取 HTTP body，不负责 Pingora 缓冲或重放。

```text
原始消息序列 → 源协议 decode → Vec<IR Message> → 业务处理 → 目标协议 encode → 原始消息序列
```

序列级边界允许把 Messages/Gemini 中混合的用户文本和工具结果拆成 Chat 的 user/tool 消息；反向转换可以把相邻兼容消息合并。拆分时保持片段次序；无法在目标协议保持顺序的组合应报错。

## 保留字段约定

IR 的 `Message.metadata` 和 `Part.metadata` 使用私有键 `_llmproxy_wire`：记录 `protocol`、原始 `form`、未被规范化的 `extra` 以及缺失与 `null` 的差异。字段以原协议 JSON 名称保存；保留项仅包含未转入 IR 的字段，不能复制整条消息或大型媒体载荷。规范化字段从 IR 编码，再补回保留字段，因此 IR 修改优先。只有回到来源协议时恢复其形状与扩展字段；跨协议编码不泄漏来源协议字段。

Gemini 省略的 `role` 映射到 `Role::Unspecified`，回到 Gemini 时继续省略；转换到其他协议时若无可靠角色信息则报错。

## 验收任务

- [x] A1：定义保留字段和错误类型；为 Gemini 未指定角色增加 IR 表示。
- [x] A2：实现 Chat、Responses、Messages、Gemini 消息序列适配器；保留原协议形状与附加字段。
- [x] A3：验证四协议原样往返、修改 IR 后重编码、跨协议拆分/合并和不支持内容的显式错误。
- [x] A4：格式、受影响 crate 测试和 workspace 完整验证。

验收：`cargo check --workspace --offline`、`cargo test --workspace --offline` 通过，工作区共 108 项测试。

参考：[Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses](https://developers.openai.com/api/reference/resources/responses/methods/create)、[Claude Messages](https://platform.claude.com/docs/en/api/http/messages)、[Gemini generateContent](https://ai.google.dev/api/generate-content)。
