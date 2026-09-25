# 网关可观测性收口与连接诊断

## 需求与范围

关联：[原始需求](00-original-requirements.md)、[实施任务](02-tasks.md)、[请求生命周期](06-pingora-request-lifecycle.md)。

本轮集中网关的 tracing、logs、metrics 记录，补齐 DNS、TCP、TLS、复用和失败诊断，继续使用现有 OTLP/HTTP 出口接入 OpenObserve 或 Collector。代理阶段负责采集事实与执行协议逻辑，可观测性模块负责字段、错误分类、日志和指标。

沿用 Pingora 原生连接器。`HttpPeer` 实现 `Peer`，包含目标、SNI、TLS 验证、超时和连接池参数；真正执行 HTTP 转发的是 Pingora。`ProxyHttp::upstream_peer` 返回具体的 `Box<HttpPeer>`，不能直接替换成 Rama 客户端。当前需求不需要替换连接器。

## 已核对的 Pingora 0.9.0 边界

| 入口 | 能获得的信息 | 注意事项 |
| --- | --- | --- |
| 应用 DNS 解析 | 耗时、选中地址、失败 | DNS 在构造 HttpPeer 前发生，失败不会经过 `fail_to_connect` |
| `Tracing::on_connected` | L4 连接成功 | 发生在 TLS 握手之前，无参数，不代表 HTTPS 已就绪 |
| `Tracing::on_disconnected` | L4 Stream 对象释放 | 不代表精确的远端断开时刻，也不必然是故障 |
| `connected_to_upstream` | 是否复用、连接句柄、Digest | 建联成功和连接池复用都会调用 |
| `Digest::timing_digest` | 已完成的 TCP、TLS 耗时 | 未测量是缺失；复用时的数据属于原连接，不能再次计作握手 |
| `Digest::ssl_digest` | TLS 版本、cipher | TLS 失败时没有成功 Digest，不能伪造握手耗时 |
| `fail_to_connect` | 建联/TLS 错误 | 结合类型和已知阶段诊断；总连接超时不等同于精确 TLS 握手超时 |
| `error_while_proxy` | 读写、协议、断流错误 | 上游 HTTP 429/500 不是连接器错误，另记 upstream_status |
| `logging` | 最终响应状态、错误、请求耗时 | 唯一请求完成出口；SSE 在结束或中断时记录 |

来源：[ProxyHttp](https://docs.rs/pingora-proxy/0.9.0/pingora_proxy/trait.ProxyHttp.html)、[HttpPeer](https://docs.rs/pingora-core/0.9.0/pingora_core/upstreams/peer/struct.HttpPeer.html)，以及锁定版本源码中的 `upstreams/peer.rs`、`connectors/l4.rs`、`protocols/digest.rs`、`protocols/tls/rustls/stream.rs`。不要把连接池持有的 tracer 绑定到某次请求的 span，否则会延长请求 span，并让后续连接释放错误地归属原请求。

## 分层与阶段分工

```text
Pingora 回调 → RequestTelemetry（请求内采集）
                     ↓ logging
              GatewayTelemetry::finish
                 ├─ 请求 span / 完成事件
                 ├─ 结构化 JSON / OTLP logs
                 └─ 低基数 metrics

PeerOptions::tracer → ConnectionProbe → 独立连接生命周期事件
observability::init → subscriber / 三类 exporter / shutdown
```

| 阶段 | 采集/动作 | 记录位置 |
| --- | --- | --- |
| `request_filter` | 创建请求标识、span，固定协议与路由、选中的上游地址 | `observability` |
| `upstream_peer` | 异步 DNS 并测时，设置 tracer，开始连接获取计时 | 请求采集对象 |
| `connected_to_upstream` | 连接复用、实际地址、TCP/TLS Digest | 请求采集对象 |
| `upstream_response_filter` | 上游状态、响应头耗时 | 请求采集对象 |
| 失败回调 | 记录失败阶段、连接获取耗时；保持原有 HTTP 映射和禁止重试 | 请求采集对象 |
| `logging` | 汇总状态、错误和耗时，结束 span，记录计数/直方图 | 唯一请求完成出口 |
| tracer 回调 | TCP 成功与对象释放；独立于请求 span | 连接观测模块 |
| 启动/快照刷新 | 启动地址、刷新失效和恢复事件 | `observability` 公共事件函数 |

DNS 改用 Tokio 异步解析，独立使用 Provider 的 `connect_timeout_ms` 作为等待上限；解析结束后 Pingora 的 TCP+TLS 总连接预算仍为该值。因此两段预算独立，不承诺 DNS+TCP+TLS 合计只使用一份预算。系统 resolver 的底层阻塞工作可能在超时后继续完成，但不会继续阻塞当前请求。

## 字段与统计约定

- 请求完成事件带 `event_kind=request`、进程内 request_id、协议、路由、最终 status、upstream_status、上游地址、连接复用、各阶段秒数、TLS 版本/cipher、错误类型/来源/阶段。请求 span 包含 method 与无 query、最多 512 字符的 path；本地 JSON 日志包含这些 span 字段，OTLP 日志可按 trace_id 关联请求 span。
- 有 OTLP tracing 时附带实际 trace_id/span_id，完成事件属于请求 span；本地无 tracer 时仍可按 request_id 查询。
- 连接 ID 在进程内唯一。tracer 不保存请求 span；通过连接句柄关联池中实际连接，释放时清除映射，避免句柄复用导致串线。
- 未发生/无法测量的阶段不填 0。`connection_acquire` 包括连接池查找、调度、TCP/TLS；`upstream_headers` 从取到连接到收到响应头，包括请求发送，不是首 token 时间。
- 请求计数/失败数/总耗时在完成时各记录一次；`error != None` 或最终 status ≥ 500 算失败。HTTP 4xx 保留状态，未自动视为传输故障。
- 阶段耗时使用同一 `llmproxy.phase.duration` 直方图，`phase` 为固定枚举；TCP/TLS 成功耗时只统计新建连接。已有 `llmproxy.requests`、`llmproxy.failures`、`llmproxy.duration` 保持可用。
- 指标仅使用固定路由、协议、状态、错误类型/阶段；不使用 request_id、trace_id、连接 ID、主机/IP、原始路径或错误文本作标签。
- API Key、Authorization、请求/响应体、query、数据库 URL 和任意错误上下文不记录。错误详情使用受控分类和可提取的 IO kind/OS code；不把整条错误链直接 Display/Debug 输出。

## 本轮与后续任务

| 编号 | 任务 | 验收 |
| --- | --- | --- |
| O1 | 需求、边界与阶段设计落文档 | 原始需求、任务、生命周期相互链接 |
| O2 | 收口 span、请求事件、指标、启动/刷新事件 | 代理和快照模块不直接调用 tracing/metrics |
| O3 | 接入 DNS、tracer、Digest、错误阶段 | DNS/连接/TLS/读写错误能定位，未知值不伪造 |
| O4 | 自动化验证 | HTTP、SSE、本地拒绝、连接复用、错误脱敏、三类遥测关联 |
| O5 | 更新运行说明并完成 workspace 验证 | 文档与行为一致，完整测试通过 |

此轮首先交付上述集中记录与诊断；[独立访问日志](04-access-logging.md)的专用过滤器、异步有界队列及丢弃计数，以及 W3C 上下文提取/传播继续单列任务。当前请求完成日志依然受 `RUST_LOG` 控制。连接生命周期事件用 DEBUG，默认请求完成记录已经包含相关诊断；无需开启所有 Pingora DEBUG 日志。

## 实现位置与验证范围

- [observability.rs](../crates/llmproxy-gateway/src/observability.rs)：subscriber、OTLP 三类出口，以及启动/快照刷新事件。
- [request.rs](../crates/llmproxy-gateway/src/observability/request.rs)：请求采集对象、统一完成事件、span 收尾、错误分类和指标。
- [connection.rs](../crates/llmproxy-gateway/src/observability/connection.rs)：Pingora tracer 适配、进程内连接身份、释放清理与生命周期计数。
- [代理回调](../crates/llmproxy-gateway/src/proxy.rs)：异步 DNS、连接参数和阶段数据采集；不直接调用 tracing 或 metrics。

新增测试覆盖实际 HTTP 错误透传、本地拒绝、无 query/凭据/请求体日志、真实连接池复用、DNS/连接拒绝、SSE 结束/中断，以及 TLS 停滞时 TCP 已连通而 TLS 数据缺失。SDK 内存 exporter 验证 trace/log 关联、请求与失败计数仅记录一次、低基数标签、连接 tracer 不延长请求 span、TLS Digest 提取和复用不重复统计握手。

TLS 成功数据使用受控 Digest 单元测试；本轮没有调用真实 Provider 或 OpenObserve，也未增加公网 TLS 成功联调。SDK 三类输出验证不等同于远端存储接收验证，OTLP 接收端内容断言仍保留在后续任务。

本轮完整验证：格式检查、workspace 编译检查和 33 项测试全部通过；新增 2 项 SDK 单元测试、4 项 HTTP/SSE 遥测集成测试，并扩展现有 TLS 超时用例的诊断断言。遥测单元测试隔离 subscriber 设置/拆除，避免并行测试共享 tracing callsite 的干扰。
