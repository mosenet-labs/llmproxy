# 架构设计

## 目标和范围

提供四个入口：`POST /v1/chat/completions`、`POST /v1/responses`、`POST /v1/messages` 和 `POST /v1/auto`。前三个入口透明代理原生协议；自动入口识别协议后代理到对应上游，不转换协议请求体或响应体。后续加入 Topcoat 管理控制台。

## 分层与工程结构

```text
客户端
  -> Pingora 接入层（方法/路径、鉴权、限制、流式代理）
  -> 应用层（协议识别、选择上游、配置快照）
  -> 领域层（协议、Provider、路由、配置校验）
  -> 基础设施（环境变量密钥、配置文件、OTLP、日志和指标）
```

`llmproxy-core` 只含类型、配置校验和纯路由/识别逻辑，不依赖 Pingora 或 Topcoat。`llmproxy-gateway` 实现 Pingora 回调和遥测。将来 `llmproxy-console` 作为独立服务接入共享配置模型；管理 API 负责保存和发布配置版本，代理请求读取不可变快照。首版使用启动时读取的 TOML 文件；热更新与持久化留到控制台阶段。

## 路由约定

| 本地路径 | 协议 | 上游路径 |
| --- | --- | --- |
| `/v1/chat/completions` | OpenAI Chat | `/v1/chat/completions` |
| `/v1/responses` | OpenAI Responses | `/v1/responses` |
| `/v1/messages` | Anthropic Messages | `/v1/messages` |
| `/v1/auto` | 依据头与 JSON 识别 | 对应的原生上游路径 |

仅允许已配置的上游主机，客户端不能指定任意目标 URL。上游凭据只通过环境变量引用，启动时验证；客户端凭据不转发到 Provider。响应状态码、错误体和 SSE 字节流保持原样。默认不缓存 LLM 响应。

## 自动识别

优先通过 `anthropic-version` 识别 Anthropic Messages；没有该头时，JSON 顶层 `input` 对应 Responses，顶层 `messages` 对应 Chat。两种信号冲突或缺失时返回 `400`。仅凭 `messages` 无法可靠区分 Chat 与 Anthropic，因此未提供 Anthropic 标识的 Messages 请求不能保证被识别；调用方可使用明确入口。

Pingora 的 `upstream_peer` 在请求体过滤器前执行，而 `request_body_filter` 每次只收到一块数据。因此自动入口必须先限量读取请求体、识别并重放，再建立上游连接；不可仅在 `request_body_filter` 中做上游选择。这个处理器作为单独任务实现，须验证同端口接入和请求体重放。明确入口继续使用 Pingora 的透明流式代理。自动入口设可配置请求体上限，超限返回 `413`。参见 [Pingora 请求阶段](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/phase.md)。

## 可观测性

统一使用 OpenTelemetry：每次请求一个 trace span，结构化日志携带 route/protocol/upstream/status/latency，指标至少包含请求量、失败量和耗时直方图。只用固定的低基数字段做指标标签；不记录完整提示词、响应体、API key 或任意模型名。默认输出本地 JSON 日志。设置 OTLP 环境变量后经 OTLP/HTTP 导出 traces、logs、metrics。OpenObserve 的服务端地址为 `https://<host>/api/<org>`，三类信号分别写到 `/v1/traces`、`/v1/logs`、`/v1/metrics`；认证头由运行环境注入。参见 [OpenObserve OTLP 文档](https://openobserve.ai/docs/ingestion/logs/otlp/)。

## 安全和错误行为

- 路由以方法和完整路径匹配，未知路径 `404`，不允许的方法 `405`。
- 请求头重写 `Host` 和 Provider 凭据，过滤逐跳头；保留必要的协议头。
- 连接超时与响应读取超时分别配置，SSE 不做全量缓冲或自动压缩。
- `POST` 请求在请求体已发出或响应已开始后不重试，避免重复生成和计费。
- 配置在启动时校验，缺少 Provider 密钥时启动失败。

## Topcoat 控制台演进

管理服务与网关分进程运行，管理端口仅暴露在可信网络。控制台读取/修改配置、查看上游健康状态及聚合指标。管理 API 验证操作身份并记录审计事件。单机阶段可用 SQLite 保存配置，发布新版本后网关原子切换不可变配置快照；多实例时再引入集中存储与通知。Topcoat 当前是 Rust 全栈框架，见[项目文档](https://github.com/tokio-rs/topcoat)。
