# 本地运行与遥测

## 控制台与网关启动

运行 `bash scripts/dev.sh up` 会编译并启动单一 `llmproxy` 进程（3200）；进程启动时检查并执行待应用的数据库迁移，完成后才加载 Provider 和模型路由、开放端口。`/ui/providers`、`/ui/models` 与 `/ui/routes` 为控制台，`/v1/*` 为代理。新增并启用 Provider 后，在 Models 页从 `/models` 探测结果选择上游模型、设置标识和至少一个协议。模型标识可直接请求；Model Routes 页只管理显式创建的对外名称，可配置多个候选模型及顺序。代理按请求体顶层 `model` 名称选择目标：缺少名称返回 `400`，未知名称或该协议无候选模型返回 `404`，路由停用或无可选目标返回 `503`。组件库由 Cargo 从 crates.io 下载，版本见 [Provider 控制台启动说明](07-provider-console.md#启动)。本机现有 `.env` 包含 PostgreSQL URL 时，仍会选择 PostgreSQL。

不设置 `LLMPROXY_DATABASE_URL` 或设置为空白时，默认使用工作目录下的 `./data/llmproxy.sqlite3`。首次启动自动建目录、生成 `llmproxy.sqlite3.key` 并执行 SQLite 迁移；后续启动沿用同一密钥。请将数据库文件和密钥文件一起备份。已有数据库缺少密钥文件会拒绝启动；显式 `LLMPROXY_MASTER_KEY` 优先于密钥文件。`sqlite::memory:` 不适用于统一服务。

使用 PostgreSQL 时显式设置 `LLMPROXY_DATABASE_URL=postgresql://...` 和固定的 `LLMPROXY_MASTER_KEY`；直接运行 `cargo run` 也会在启动时执行待应用的迁移。迁移或主密钥校验失败时不会开始监听。`bash scripts/dev.sh migrate` 仍可用于单独执行迁移。显式地址连接失败不会回退到 SQLite。两种数据库数据彼此独立，不提供跨后端迁移。默认监听 `127.0.0.1:3200`。当前自动入口尚未实现完整代理，返回 `501`；具体任务见[实施任务](02-tasks.md)。

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

网关测试使用本地 TCP 模拟上游和自动启动的网关进程，不需要真实 Provider API key 或 OpenObserve。默认使用独立临时 SQLite 文件运行控制台、明确入口、SSE 和可观测性集成测试，不会清空开发数据库。设置 `LLMPROXY_TEST_DATABASE_URL` 时，额外运行隔离 PostgreSQL schema 的回归测试；该 URL 必须指向测试数据库。运行环境需要允许监听和访问回环地址。完整行为约定与测试矩阵见[三个明确入口的联调验收](05-explicit-routes-validation.md)。

三个明确入口必须发送未压缩 JSON，顶层 `model` 字段填写 Model Routes 页配置的路由名称。它须完整落在请求体前 64 KiB；后续正文保持流式。网关只改写上游请求的 `model` 值，响应错误和 SSE 原样转发。具体行为见[Model Routes](18-model-routes.md)。

## OTLP/OpenObserve

统一入口启动时通过 `dotenvy` 加载当前目录或父目录的 `.env`，已存在的进程环境变量优先。通过 `llmproxy-telemetry` 初始化一次遥测，默认将结构化 JSON 日志写到 stdout。可在共享 `.env` 中配置：

```dotenv
OTEL_SERVICE_NAME=llmproxy
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=http://<host>:5080/api/<org>
OTEL_EXPORTER_OTLP_HEADERS="authorization=Basic%20<base64(email:password)>,stream-name=llmproxy"
OTEL_EXPORTER_OTLP_TIMEOUT=10000
RUST_LOG=warn,llmproxy=info,llmproxy_console=info,llmproxy_telemetry=info
```

统一使用 `OTEL_SERVICE_NAME`，缺省或空白时为 `llmproxy`。三类遥测共用同一 Resource，日志通过同一 `stream-name` 写入 OpenObserve。`component=console/gateway` 区分模块；旧的两个 `LLMPROXY_*_SERVICE_NAME` 和 `LLMPROXY_CONSOLE_PORT` 已移除。详见[单端口集成](11-unified-service.md)。

不要在配置文件或仓库中放置实际凭据。OpenObserve 会在基地址下接收 `/v1/traces`、`/v1/logs`、`/v1/metrics`。如使用 Collector，也可把变量指向 Collector 的 OTLP/HTTP 地址。开发环境没有 OTLP 地址时，仅启用本地日志；指标调用不会导出。

参考：[OpenObserve OTLP 接入](https://openobserve.ai/docs/ingestion/logs/otlp/)。

控制台请求日志包含 `request_id`、`method`、受控 `route`、`status` 和 `duration_ms`；Provider 操作日志包含 `action`、`provider_id`、`provider_name`、`outcome`，失败时附带受控 `error_kind`。OTLP 开启时两类事件可按 trace/span ID 关联。请求耗时截止响应对象构造完成。指标为 `llmproxy.console.requests`、`llmproxy.console.request.duration`（秒）与 `llmproxy.console.provider.operations`，不使用 Provider 名称或 ID 作为标签。

## 网关连接诊断

网关的 span、完成日志、阶段指标和连接事件集中在 `observability` 模块。默认每个请求在结束时输出一条 `event_kind=request` 的 JSON 完成记录，含 DNS/连接获取/TCP/TLS/响应头耗时、复用标志、连接 ID、TLS 版本与 cipher、上游状态和错误阶段。SSE 中途失败时保留已发送状态，并记录 `failed=true`。缺失阶段不填 0。

设置 OTLP 后，请求日志附带 `trace_id` / `span_id`，OpenObserve 中可据此关联 trace；默认本地日志仍可按进程内 `request_id` 查询。`llmproxy.phase.duration` 按固定 `phase` 标签统计各阶段耗时，复用连接不新增 TCP/TLS 握手样本。`llmproxy.connection.events` 的 `event=connected/released` 分别计数 L4 成功与对象释放。

排查连接生命周期时可设置 `RUST_LOG=llmproxy=info,llmproxy::observability::connection=debug,llmproxy_console=info,pingora=warn`；DEBUG 事件独立于请求 span，通过 `connection_id` 关联。`error_stage=dns/connect/tls/response_headers/response_body/request_write/downstream/request` 表示受控诊断分类。总连接超时仍归 `connect`，不能据此声称测得 TLS 失败耗时。

这些完成事件仍受 `RUST_LOG` 控制，独立访问日志出口与 W3C 上下文传播尚待后续任务。详见[可观测性设计](08-gateway-observability.md)。

## Topcoat 0.10 开发工具与迁移

安装匹配的 CLI 与 rustfmt：

```sh
cargo install topcoat-cli --version 0.10.0 --locked
rustup component add rustfmt
topcoat --version
cargo topcoat --version
bash scripts/fmt.sh
bash scripts/fmt.sh --check
```

`rustfmt.toml` 指定 Rust 2024；`rust-analyzer.toml` 使用 `topcoat fmt --stdin --rustfmt` 格式化 Rust 与宏。CI 可运行 `bash scripts/fmt.sh --check`，差异或错误返回非零。CLI 构建保留 Cargo/Rust 环境设置。网关设置 `default-run = "llmproxy"`，运行目标不再依赖二进制名称推断；统一服务仍通过 `scripts/dev.sh` 启动。

控制台内部页面链接使用 `link_attrs` 实现无刷新导航，默认 Intent 预取。聊天页面初始化可能创建会话，因此入口设置 Never；订阅状态的显式刷新也使用 Never，避免复用提前读取的状态。聊天恢复脚本在全部页面加载，并观察导航插入的工作区，每个工作区只触发一次恢复。

模型候选使用 `#[record]` 和 `Signal<Vec<ModelCandidate>>` 直接传输；别名与聊天草稿使用 `Signal<Vec<(String, String)>>`，模型重复检查直接捕获元组集合，历史操作返回类型化元组。捕获的 record 仅含已公开的模型 ID 和参考价格，不包含 Provider 密钥；写入仍经过服务端验证。

逐项适用性：

- 客户端导航、预取、record、元组、格式化/检查、CLI 版本和 default-run 已接入。
- runtime 脚本继续由 `runtime::script()` 生成，包含 `data-topcoat-usize-bits`；项目未使用依赖浮点字符串长度的表达式。
- 模块路由未使用 `path_param!` 声明动态模块，无需替换为 `module_param!`。
- 当前 Pingora HTTP 适配器不提供 Hyper WebSocket upgrade，聊天继续通过 HTTP 流式 shard 更新。共享 WebSocket 的运行时改进随依赖升级获得，但不能在此适配器上直接调用 `connected(cx)`；连接渲染上限显式保持框架默认 64。
- 本项目使用独立组件库而非 Topcoat UI registry，不运行 `ui add --all`，以免引入另一套组件和覆盖现有样式。
- Cargo 子命令修复、流式开发刷新、宏补全、thiserror 兼容和确定性宏输出由框架升级获得，无需业务代码模拟。
- 框架 `docs/` 与 `llms.txt` 是上游文档变化；项目保留现有文档目录和 AGENTS.md。
