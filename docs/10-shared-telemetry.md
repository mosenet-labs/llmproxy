# 控制台与网关统一可观测性

## 当前方案

`llmproxy` 已按[单进程、单端口方案](11-unified-service.md)集成网关与 `/ui` 控制台，启动时只初始化一次 tracing 和 OTLP exporters。上一轮双进程的专属服务名配置已经移除。

| 模块 | 职责 |
| --- | --- |
| `llmproxy-telemetry` | 解析服务名，初始化 JSON 日志、subscriber 和三类 OTLP/HTTP exporter，正常退出时刷新 |
| 网关 `observability` | Pingora 阶段、连接诊断、请求日志和指标；`component=gateway` |
| 控制台 `observability` | Topcoat 请求 layer、Provider 操作、请求 span 与指标；`component=console` |

## .env 配置

```dotenv
LLMPROXY_LISTEN=127.0.0.1:3200
OTEL_SERVICE_NAME=llmproxy
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=http://<openobserve-host>:5080/api/<org>
OTEL_EXPORTER_OTLP_HEADERS="authorization=Basic%20<base64凭据>,stream-name=llmproxy"
OTEL_EXPORTER_OTLP_TIMEOUT=10000
```

服务名取 `OTEL_SERVICE_NAME`，缺省或空白时为 `llmproxy`。三类遥测使用同一 Resource。`stream-name` 是 OpenObserve 的日志流，与 `service.name` 独立；共用日志流后按 `component` 筛选模块。

移除旧的 `LLMPROXY_GATEWAY_SERVICE_NAME`、`LLMPROXY_CONSOLE_SERVICE_NAME`、`LLMPROXY_CONSOLE_PORT`；不增加两个日志流变量。

参考：[OTEL 标准配置](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)、[OpenObserve OTLP 接入](https://openobserve.ai/docs/ingestion/logs/otlp/)。

## 记录边界

- 控制台的全局请求 layer 覆盖页面、原生异步接口、403/404/405/413；内部 page rewrite 交回框架继续处理。每次 HTTP 请求输出一条完成事件，携带请求 ID、受控 method/route、状态和处理耗时。有有效 OTEL 上下文时附带 trace ID/span ID。
- 控制台耗时截止响应对象构造完成，不代表浏览器收完数据。传输层拒绝或中断使用单独的受控 transport 事件。UI 请求不再重复记为网关请求。
- Provider 六类写操作在共用业务入口记录一次，包含 ID、名称、操作与结果；业务失败即使 HTTP 为 200，也有失败分类。
- 指标只使用固定 route/method/status/component/action/outcome/error_kind 等低基数标签。Provider 名称与 ID 仅进入日志。
- 不记录 API Key、CSRF、数据库 URL、主密钥或原始数据库错误；控制台原始查询串、动态路径和请求体不进入日志。
- 日志受 `RUST_LOG` 控制。独立访问日志、W3C 跨服务传播仍按原有任务推进。

## 验证

本地接收端解码 traces、logs、metrics 的 OTLP protobuf，验证统一 Resource、日志与 trace 关联及退出刷新；HTTP/PG 与浏览器验证见[集成验收](11-unified-service.md)。不向真实 OpenObserve 写入测试数据。
