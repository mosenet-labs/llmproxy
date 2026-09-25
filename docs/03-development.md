# 本地运行与遥测

## 控制台与数据库模式

本轮本机已准备 `llmproxy_dev`、`llmproxy_test` 和忽略提交的 `.env`。运行 `bash scripts/dev.sh up` 会编译、迁移数据库，并同时启动控制台（3200）和网关（8080）。新增 Provider 后选择“设为当前”；没有绑定的协议入口返回 `503`。从其他机器克隆时按 [Provider 控制台启动说明](07-provider-console.md#启动) 准备数据库、组件库和环境变量。

## TOML 模式启动

未设置 `LLMPROXY_DATABASE_URL` 时，复制 `config/gateway.example.toml`，并为其中的 `api_key_env` 设置环境变量。启动示例：

```sh
export OPENAI_API_KEY='...'
export ANTHROPIC_API_KEY='...'
LLMPROXY_CONFIG=config/gateway.example.toml cargo run -p llmproxy-gateway
```

网关默认监听 `127.0.0.1:8080`。配置文件不包含真实密钥。当前自动入口尚未实现完整代理，返回 `501`；具体任务见[实施任务](02-tasks.md)。

## Provider 超时

每个 Provider 可配置 `connect_timeout_ms`、`read_timeout_ms`、`write_timeout_ms`，省略时分别使用 10000、60000、30000 毫秒，均必须大于 0。连接超时覆盖 TCP 与 TLS 握手；读取超时针对上游读取操作，不限制整个 SSE 流的总时长。

尚未开始响应时，上游超时返回 `504`，其他上游传输故障返回 `502`。流开始后的故障会终止流，保持已经发送的状态码。Provider 自己返回的 HTTP 错误保留原状态和错误体。

写入请求体超时后，Pingora 仍可能等待上游响应；该等待由读取超时约束，完整的上游响应仍可透传。DNS 使用异步解析，单独以 `connect_timeout_ms` 限制等待；随后 TCP+TLS 建联另有一份相同预算。系统 resolver 的底层工作可能在等待超时后继续完成。详细边界见[超时契约](05-explicit-routes-validation.md#超时契约)。

## 自动化联调

```sh
cargo test --workspace
# 只运行三个明确入口的集成测试
cargo test -p llmproxy-gateway --test explicit_routes
```

网关测试使用本地 TCP 模拟上游和自动启动的网关进程，不需要真实 API key 或 OpenObserve；测试进程会清理临时文件和子进程。数据库相关测试需要显式导出 `LLMPROXY_TEST_DATABASE_URL`，在独立 schema 中执行并清理，不会清空数据库；未设置时会跳过数据库测试。可先 `set -a; source .env; set +a` 再运行测试。运行环境需要允许监听和访问回环地址。完整行为约定与测试矩阵见[三个明确入口的联调验收](05-explicit-routes-validation.md)。

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

## 网关连接诊断

网关的 span、完成日志、阶段指标和连接事件集中在 `observability` 模块。默认每个请求在结束时输出一条 `event_kind=request` 的 JSON 完成记录，含 DNS/连接获取/TCP/TLS/响应头耗时、复用标志、连接 ID、TLS 版本与 cipher、上游状态和错误阶段。SSE 中途失败时保留已发送状态，并记录 `failed=true`。缺失阶段不填 0。

设置 OTLP 后，请求日志附带 `trace_id` / `span_id`，OpenObserve 中可据此关联 trace；默认本地日志仍可按进程内 `request_id` 查询。`llmproxy.phase.duration` 按固定 `phase` 标签统计各阶段耗时，复用连接不新增 TCP/TLS 握手样本。`llmproxy.connection.events` 的 `event=connected/released` 分别计数 L4 成功与对象释放。

排查连接生命周期时可设置 `RUST_LOG=llmproxy_gateway=info,llmproxy_gateway::observability::connection=debug,pingora=warn`；DEBUG 事件独立于请求 span，通过 `connection_id` 关联。`error_stage=dns/connect/tls/response_headers/response_body/request_write/downstream/request` 表示受控诊断分类。总连接超时仍归 `connect`，不能据此声称测得 TLS 失败耗时。

这些完成事件仍受 `RUST_LOG` 控制，独立访问日志出口与 W3C 上下文传播尚待后续任务。详见[可观测性设计](08-gateway-observability.md)。
