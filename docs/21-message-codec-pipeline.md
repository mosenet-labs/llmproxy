# 同协议消息 IR 处理链路

## 范围与约束

首阶段只处理客户端与 Provider 使用同一协议的 JSON 请求和非流式 JSON 响应。完整报文继续由 `serde_json::Value` 承载；只将已经建模的 message 节点交给四协议现有的请求、响应适配器。没有消息节点的合法报文、错误响应和 SSE 保持原样。当前没有消息编辑规则，生产链路只完成消息投影；原始字节保持不变。

协议选择仍在 Pingora `request_filter` 完成，模型别名仍在该阶段预读和改写。`request_body_filter` 收齐请求 JSON 后，以客户端协议读取消息；`upstream_response_filter` 根据响应头选择 JSON 或 SSE，`upstream_response_body_filter` 收齐非流式 JSON 后，以 Provider 协议读取消息。现有路由按 `(协议, 模型别名)` 选择 Provider，本阶段目标协议与客户端协议相同。网关不把不完整的消息 IR 当作完整报文发送给其他协议。

## 公共契约

`adapter::codec::MessageCodec` 定义请求和响应两组 `decode/encode` 操作，输入或输出为 `Value` 外壳与对应的消息 IR。解码产物同时记录消息在外壳中的位置；编码只回填这些位置，不动请求参数、其他输出项、候选元数据等。消息数量变化在本阶段明确拒绝，以免丢失 Responses `input/output` 中穿插的非消息项。没有 IR 编辑时保留整份原始字节；发生编辑后，未选中的节点保留 JSON 语义，但重新序列化可能改变空白、字段顺序和数字写法。各协议的 DTO 及转换仍由原 `adapter/request|response/<protocol>` 实现；公共契约只负责定位和调度。

当未来加入编辑规则时，只有 IR 实际变化才序列化新的 JSON 正文，同时必须处理请求和响应的 `Content-Length`、压缩及缓存体积边界。跨协议还需要外层参数、路径、头和响应结构的映射；SSE 需要独立的事件级契约。

## 任务与验收

- [x] C1：实现公共消息 codec 与四协议 JSON message 定位；混合数组中的非 message 项保持位置和内容。
- [x] C2：接入网关请求与非流式响应的 JSON 读取路径；没有编辑规则时保持原始字节及 HTTP 头行为。
- [x] C3：验证四协议的消息投影和编辑后回填，验证 Responses 混合项、多 choice/candidate、无消息、错误响应与 SSE 透传。
- [x] C4：完成受影响测试、格式、Clippy 和 workspace 验证。

验证：`cargo fmt --all`、workspace Clippy `-D warnings` 通过；workspace 测试 125 项通过，包含本地 HTTP/SSE 和四协议消息编解码用例。
