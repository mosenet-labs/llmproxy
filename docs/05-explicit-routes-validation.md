# 三个明确入口：阶段分工与联调验收

关联：[原始需求](00-original-requirements.md)、[架构设计](01-architecture.md)、[实施任务](02-tasks.md)、[本地运行](03-development.md)。

各阶段怎样串联、返回值如何控制流程，以及 SSE 和错误路径的说明，见[Pingora 请求生命周期](06-pingora-request-lifecycle.md)。

## 范围与行为约定

本任务验证 `POST /v1/chat/completions`、`POST /v1/responses`、`POST /v1/messages` 的透明代理行为。三个入口共用 Pingora `ProxyHttp` 流程，通过请求上下文中的协议选择 Provider。测试使用本地模拟上游和实际网关进程。

- 请求路径、查询参数、请求体保持原样，不解析或重新序列化明确入口的 JSON。
- Host 来自 Provider 配置；HTTP 非 80、HTTPS 非 443 端口写入 Host。TLS SNI 使用主机名，不包含端口。
- 清除客户端所有 `Authorization`、`x-api-key` 值，然后注入对应 Provider 凭据。
- Anthropic 配置了 `anthropic_version` 时覆盖客户端值；未配置时保留客户端值。两者均缺失时由上游返回协议错误。`anthropic-beta`、`Content-Type`、`Accept` 等端到端头保留。逐跳头按 HTTP 代理语义处理。
- 上游业务错误（包括 400、401、429、500、503）保留状态码、原始响应体及 `Retry-After`、Provider request ID 等端到端响应头，不转为网关统一错误。
- SSE 按字节流转发，不聚合整个响应、不自动压缩、不合成完成事件；不承诺 TCP/HTTP chunk 边界相同。
- 未知路径本地返回 404；已知路径的非 POST 请求返回 405 和 `Allow: POST`；两者都不访问上游。自动入口暂时保留 501。

## Pingora 0.9 阶段分工

| 阶段 / Hook | 功能 | 本任务职责与后续扩展 |
|---|---|---|
| `new_ctx` | 请求上下文 | 初始化计时器、协议、span 等状态 |
| `early_request_filter` | 早期追踪 | 后续提取 traceparent、生成 request ID；本任务保留现有追踪入口 |
| `request_filter` | 路由及本地拒绝 | 选择三个明确协议；生成 404/405/501；本地响应后返回 `Ok(true)` |
| `upstream_peer` | Provider、TLS、超时 | 创建 HttpPeer，设置目标地址、端口、SNI，以及连接和读写超时 |
| `connected_to_upstream` | 连接观测 | 后续访问日志记录连接地址及复用信息 |
| `upstream_request_filter` | 请求头 | 改写 Host、凭据及配置控制的协议头；后续注入 trace 上下文 |
| `request_body_filter` | 请求体 | 保持默认流式透传 |
| `upstream_response_filter` | 上游响应头 | 保留状态与端到端响应头，清理逐跳头；后续记录上游状态、响应头耗时 |
| `response_filter` | 下游响应头 | 保持默认代理行为；后续添加公共响应头。本地直接响应须由响应函数设置公共头 |
| `upstream_response_body_filter` / `response_body_filter` | 响应体及 SSE | 保持默认流式透传；按需扩展首块到达观测 |
| `fail_to_connect` | 建连故障 | 保留错误类型，禁止本阶段自动重试，避免重复尝试造成超时语义不明确 |
| `error_while_proxy` | 转发故障 | 禁止自动重试；本项目明确入口均为生成类 POST |
| `fail_to_proxy` | 网关错误响应 | 按错误来源与类型处理 502/504；流开始后终止，禁止追加第二个响应 |
| `logging` | 请求结束 | 更新现有日志、计数和耗时；记录真实状态与故障类型，未发送响应时不虚构 502 |

配置解析、环境变量凭据读取和 OTLP exporter 初始化在服务启动时完成。独立访问日志、W3C trace 传播按任务文档另行实施。

参考：[Pingora 请求阶段](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/phase.md)。实际 API 以 Cargo.lock 锁定的 0.9 源码为准。

## 超时契约

每个 Provider 增加以下可选配置，省略时使用默认值；0 为非法值：

| 配置 | 默认值 | 含义 |
|---|---|---|
| `connect_timeout_ms` | 10000 | TCP 建连超时，同时设置包含 TLS 握手的总连接超时 |
| `read_timeout_ms` | 60000 | 上游响应头/响应体的读取操作超时 |
| `write_timeout_ms` | 30000 | 向上游写请求的操作超时 |

读取超时不是整次 SSE 请求的总时限，也不是严格的首 token deadline。连接池 `idle_timeout` 不用于 SSE 无数据超时。同步 DNS 解析不受 HttpPeer 的连接超时覆盖，异步 DNS 与独立解析超时后续处理。

Pingora 在请求体写入超时后仍可等待上游响应，该等待受读取超时约束。如果上游返回完整响应，保留该响应；如果随后读取也失败，则由保存的写入错误进入故障处理。因此写入超时不等于整次请求立即返回 504 的期限。

- 上游连接拒绝、TLS 错误或非超时传输故障：尚未发送响应时返回 502。
- 上游连接/握手、读取、写入超时：尚未发送响应时返回 504。
- 已发送最终响应头后发生超时、断流：结束失败流，保留已经发送的状态，记录错误；不追加 JSON 或 SSE 完成标记。
- 下游故障按下游语义处理，不误报为上游 504。

响应中的 `Connection` 提名头及标准逐跳头会清理；`Transfer-Encoding` 保留给 Pingora 管理。如果上游通过 `Connection` 提名 Content-Length、Transfer-Encoding 或 Content-Encoding，则作为异常上游响应返回 502，避免改变消息边界或编码含义。DNS 解析失败进入 502 故障路径，不触发 HttpPeer 构造时的 panic。

## 自动化验收矩阵

| 场景 | 关键断言 |
|---|---|
| 三入口普通响应 | 各自命中不同模拟上游；路径、query、body、Host、凭据和协议头正确 |
| 重复凭据头 | 客户端多个大小写不同的凭据值全部清除；上游仅收到配置凭据 |
| Anthropic 版本 | 配置覆盖；无配置保留客户端值 |
| 上游 400/401/429/500/503 | 状态、原始 body、Content-Type、Retry-After、request ID 保留；上游收到一次请求 |
| 响应逐跳头 | 清除 Connection 提名字段和 Keep-Alive；拒绝提名消息边界/编码字段的异常响应 |
| 三协议 SSE | mock 发出并 flush 第一事件，客户端确认收到后才允许发送后续事件，证明没有全量缓冲 |
| SSE 分片 | 拆开单个事件、合并多个事件，内容和顺序保持一致 |
| SSE 长流 | 总时长超过读取超时，各次读取间隔小于超时，仍正常结束 |
| 等待响应头超时 | 上游收到请求后不发响应，网关返回 504 |
| SSE 中途停顿/截断 | 已收到 200 和事件后失败；响应不完整，无伪造完成事件，无重试 |
| 客户端取消 | 首事件后关闭客户端，上游在有限时间内观察取消/连接关闭 |
| 连接拒绝 | 返回 502，进程保持可用 |
| TLS 握手停滞 | 使用受控本地服务，验证 SNI 与总连接超时，避免公网不可达地址造成不确定性 |
| 上游写入停滞 | 上游不读取大请求体，触发写入超时；返回 504 且不重试 |
| 未知路径及错误方法 | 404/405，405 带 Allow；上游请求数为零 |

## 实施步骤与运行

1. 落地本文及关联链接。
2. 添加超时配置与校验，补齐 Host、405、错误映射和重试约束。
3. 添加本地模拟 Provider、实际网关进程的集成测试；测试负责临时配置、测试凭据、就绪检查和进程清理。
4. 执行格式、编译及完整测试，将实际覆盖与限制更新到本文和任务文档。

```sh
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
```

本地模拟测试不需要真实 Provider API key。真实 Provider 兼容性与外部 OpenObserve 接收效果不由模拟测试证明。

## 执行记录

2026-09-24：实现及本地自动化验收完成。

- `cargo fmt --all --check`：通过。
- `cargo check --workspace --locked --offline`：通过。
- `cargo test --workspace --locked --offline`：通过，6 个 core 单元测试、12 个 gateway 集成测试；各入口通过循环覆盖多种响应状态及流式场景。
- 测试入口：[explicit_routes.rs](../crates/llmproxy-gateway/tests/explicit_routes.rs)；本地进程与 HTTP 测试辅助：[support/mod.rs](../crates/llmproxy-gateway/tests/support/mod.rs)。
- 测试使用标准库，无新增依赖；清空子进程环境并注入虚拟凭据，避免继承真实 Provider key 或 OTLP exporter 配置。
- 默认并发执行曾发现临时端口预留与网关启动间的竞态，已通过统一分配锁，以及 HTTP 响应和目标子进程唯一请求日志联合就绪检查解决。业务测试请求不重试。
- SSE 断流断言检查实际传输错误，排除读取超时造成的假通过；客户端取消仅接受连接关闭类错误；写入超时校验日志中的 `WriteTimedout`。

验收边界：TLS 用例验证 ClientHello SNI 和握手停滞超时，未覆盖完整 TLS 服务端握手或真实 Provider 的网络环境；本轮没有调用真实 Provider 或 OpenObserve。DNS 仍使用同步解析，超时边界见上文。遥测出口断言、W3C trace 传播及完整访问日志继续按[任务文档](02-tasks.md)实施。
