# 本地运行与遥测

## 启动

复制 `config/gateway.example.toml`，并为其中的 `api_key_env` 设置环境变量。启动示例：

```sh
export OPENAI_API_KEY='...'
export ANTHROPIC_API_KEY='...'
LLMPROXY_CONFIG=config/gateway.example.toml cargo run -p llmproxy-gateway
```

网关默认监听 `127.0.0.1:8080`。配置文件不包含真实密钥。当前自动入口尚未实现完整代理，返回 `501`；具体任务见[实施任务](02-tasks.md)。

## Provider 超时

每个 Provider 可配置 `connect_timeout_ms`、`read_timeout_ms`、`write_timeout_ms`，省略时分别使用 10000、60000、30000 毫秒，均必须大于 0。连接超时覆盖 TCP 与 TLS 握手；读取超时针对上游读取操作，不限制整个 SSE 流的总时长。

尚未开始响应时，上游超时返回 `504`，其他上游传输故障返回 `502`。流开始后的故障会终止流，保持已经发送的状态码。Provider 自己返回的 HTTP 错误保留原状态和错误体。

写入请求体超时后，Pingora 仍可能等待上游响应；该等待由读取超时约束，完整的上游响应仍可透传。同步 DNS 解析目前不受上述连接超时覆盖。详细边界见[超时契约](05-explicit-routes-validation.md#超时契约)。

## 自动化联调

```sh
cargo test --workspace
# 只运行三个明确入口的集成测试
cargo test -p llmproxy-gateway --test explicit_routes
```

测试使用本地 TCP 模拟上游和自动启动的网关进程，不需要真实 API key 或 OpenObserve；测试进程会清理临时文件和子进程。运行环境需要允许监听和访问回环地址。完整行为约定与测试矩阵见[三个明确入口的联调验收](05-explicit-routes-validation.md)。

## OTLP/OpenObserve

默认将结构化 JSON 日志写到 stdout。设置以下变量可将 traces、logs 和 metrics 经 OTLP/HTTP 发往 OpenObserve：

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT='https://<host>/api/<org>'
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Basic <base64(email:password)>'
export OTEL_SERVICE_NAME='llmproxy-gateway'
export RUST_LOG='llmproxy_gateway=info,pingora=warn'
```

不要在配置文件或仓库中放置实际凭据。OpenObserve 会在基地址下接收 `/v1/traces`、`/v1/logs`、`/v1/metrics`。如使用 Collector，也可把变量指向 Collector 的 OTLP/HTTP 地址。开发环境没有 OTLP 地址时，仅启用本地日志；指标调用不会导出。

参考：[OpenObserve OTLP 接入](https://openobserve.ai/docs/ingestion/logs/otlp/)。
