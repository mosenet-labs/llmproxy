# 本地运行与遥测

## 启动

复制 `config/gateway.example.toml`，并为其中的 `api_key_env` 设置环境变量。启动示例：

```sh
export OPENAI_API_KEY='...'
export ANTHROPIC_API_KEY='...'
LLMPROXY_CONFIG=config/gateway.example.toml cargo run -p llmproxy-gateway
```

网关默认监听 `127.0.0.1:8080`。配置文件不包含真实密钥。当前自动入口尚未实现完整代理，返回 `501`；具体任务见[实施任务](02-tasks.md)。

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
