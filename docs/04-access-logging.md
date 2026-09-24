# 访问日志设计

## 目标

像 Nginx 或 Envoy 一样，为**每个已进入代理处理流程的 HTTP 请求**输出一条访问记录，包括成功、上游错误和网关本地返回。默认使用 JSON Lines，便于 stdout 收集或通过 OTLP 导入 OpenObserve。诊断日志、访问日志和指标使用不同的事件类别，不依靠普通业务日志的 `RUST_LOG` 级别控制访问日志。

Pingora 官方将 `ProxyHttp::logging()` 定义为请求完成或发生致命错误后的访问日志/指标收口点；它也会在本地提前返回后运行。参考 [Pingora logging 阶段](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/phase.md)和[官方访问日志示例](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/modify_filter.md)。

## 生命周期

1. `new_ctx` / `request_filter`：保存墙上时间作为显示时间戳，以 `Instant` 保存耗时起点；捕获方法、匹配后的路由模板、请求 ID 和受信任的客户端地址。
2. `upstream_peer`：保存选中的 Provider 标识及上游地址。选不到上游时仍保留请求上下文。
3. `upstream_response_filter`：仅在第一次收到最终响应头时记录上游状态码和响应头时间。不要把连接池复用时的 TLS 建连耗时当作每次请求的连接耗时。
4. `logging`：读取实际写给客户端的状态、请求/响应体字节数和最终错误，计算总耗时，构造一个不可变的 `AccessRecord` 并提交到日志出口。SSE 在流结束或中断时才写最终访问记录。

`logging` 必须是唯一的访问日志写入位置；其他回调只更新 `CTX`。不要在 `fail_to_proxy` 再写一条，避免错误请求重复记录。

## 字段约定

| 字段 | 来源和口径 |
| --- | --- |
| `timestamp` | 请求进入时的 UTC 时间，RFC 3339 |
| `request_id`, `trace_id` | 网关生成/校验后的请求 ID、当前 trace ID |
| `client_ip` | `session.client_addr()`；若使用 `X-Forwarded-For`，仅信任配置中的前置代理 |
| `method`, `route`, `path` | 方法、固定路由模板、无查询串的路径 |
| `protocol`, `upstream` | 识别后的协议、配置中的 Provider 标识或主机 |
| `status`, `upstream_status` | 实际下游状态、上游最终状态；未收到响应时为 `null`，不伪造 `502` |
| `request_body_bytes`, `response_body_bytes` | Pingora `body_bytes_read/sent()`；是应用层 body 字节，**不含 HTTP 头和线缆协议开销** |
| `duration_ms`, `upstream_header_ms` | 总耗时、上游从请求发送到响应头到达的耗时；未进入对应阶段时为 `null` |
| `error_type` | 经过归类的错误名称，避免写入可能含敏感信息的完整错误链 |

示例：

```json
{"timestamp":"2026-09-24T03:00:00Z","kind":"access","request_id":"req-123","trace_id":"4bf92f3577b34da6a3ce929d0e0e4736","client_ip":"192.0.2.10","method":"POST","route":"/v1/messages","path":"/v1/messages","protocol":"anthropic_messages","upstream":"api.anthropic.com:443","status":200,"upstream_status":200,"request_body_bytes":1024,"response_body_bytes":4096,"duration_ms":1870,"upstream_header_ms":420,"error_type":null}
```

这些 body 字节数对应 Pingora 的应用数据口径，不能直接标为 Nginx 的 `$bytes_sent` 或 Envoy 的线缆字节数。Nginx 和 Envoy 都允许自定义访问日志格式，因此我们采用语义明确的字段名，而不是强行模拟其默认格式。[Nginx 日志模块](https://nginx.org/en/docs/http/ngx_http_log_module.html)、[Envoy 访问日志](https://www.envoyproxy.io/docs/envoy/latest/configuration/observability/access_log/usage.html)

## 输出路径

建议使用 `target = "llmproxy.access"` 的专用结构化事件，单独配置 `tracing_subscriber` 的 per-layer filter，使访问事件不受普通 `RUST_LOG` 影响。当前工程将 `EnvFilter` 放在整个 subscriber 上层，若环境中 `RUST_LOG=warn`，现有 `INFO` 完成事件会消失；实现时需要把普通诊断日志过滤器移动到对应 layer，并给访问日志 layer 固定启用规则。[tracing-subscriber per-layer filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/index.html)

生产环境优先输出一行 JSON 到 stdout，由部署平台收集；也可经现有 OTLP logs exporter 发送到 OpenObserve。配置必须避免 stdout 收集器和直接 OTLP 同时导入同一条访问事件。输出需使用非阻塞、有界队列，记录丢弃计数并告警；如果要求审计级别零丢失，应另设持久化缓冲方案，不能在代理回调里同步访问磁盘或网络。[tracing-appender 队列行为](https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/index.html)

## 隐私和验证

不记录 `Authorization`、`x-api-key`、完整 URL 查询串、请求/响应 body、原始提示词。`User-Agent` 如需记录，应限制长度并使用 JSON 转义。指标标签只使用协议、路由模板、状态码等有限集合，避免请求 ID 进入指标。

验收场景：正常请求、本地 `404/405/501`、上游连接失败、上游 `5xx`、SSE 正常结束和客户端中断，均恰好一条；JSON 可解析；`RUST_LOG=warn` 不影响访问日志；记录中没有凭据；队列满时丢弃指标递增。当前 [proxy.rs](../crates/llmproxy-gateway/src/proxy.rs) 的 `logging()` 只输出简短完成事件，尚未实现此设计。
