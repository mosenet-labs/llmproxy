# 架构设计

## 目标和范围

提供四个入口：`POST /v1/chat/completions`、`POST /v1/responses`、`POST /v1/messages` 和 `POST /v1/auto`。前三个入口透明代理原生协议；自动入口识别协议后代理到对应上游，不转换协议请求体或响应体（HTTP 自动入口仍待实现）。Topcoat 控制台通过 SQLite 或 PostgreSQL 管理 Provider。

## 分层与工程结构

```text
客户端
  -> Pingora 接入层（方法/路径、鉴权、限制、流式代理）
  -> 应用层（协议识别、选择上游、配置快照）
  -> 领域层（协议、Provider、路由、配置校验）
  -> 基础设施（Toasty/SQLite/PostgreSQL、凭据加密、环境配置、OTLP、日志和指标）
```

`llmproxy-core` 只含类型、配置校验和纯路由/识别逻辑，不依赖 Pingora 或 Topcoat。`llmproxy-gateway` 实现 Pingora 回调和遥测。`llmproxy-console` 是进程内的 Topcoat 路由库，复用 `topcoat-ant-design` 构建 Provider 管理页面；`llmproxy-store` 封装 Toasty 模型、双后端迁移、事务和密钥加密。控制台写入数据库，网关每秒加载并原子切换不可变快照，请求处理不查询数据库。

所选数据库是唯一的 Provider 来源；缺省使用持久化 SQLite，显式配置 PostgreSQL 时继续使用其独立数据。每种协议允许多个 Provider，但只有一个当前绑定；在途请求持有原 Provider 快照。具体设计见 [Provider 管理控制台](07-provider-console.md)和[数据库兼容方案](12-sqlite-compatibility-plan.md)。

统一入口由 `llmproxy-gateway` 的 `llmproxy` 二进制承载，Pingora 在请求过滤阶段将 `/ui` 交给 Topcoat，其余按代理路径分发。默认 `127.0.0.1:3200`，见[单端口设计](11-unified-service.md)。

## 路由约定

| 本地路径 | 协议 | 上游路径 |
| --- | --- | --- |
| `/v1/chat/completions` | OpenAI Chat | `/v1/chat/completions` |
| `/v1/responses` | OpenAI Responses | `/v1/responses` |
| `/v1/messages` | Anthropic Messages | `/v1/messages` |
| `/v1/auto` | 依据头与 JSON 识别 | 对应的原生上游路径 |

仅允许已配置的上游主机，客户端不能指定任意目标 URL。上游凭据由控制台录入并加密保存，SQLite 默认从配套私有密钥文件读取主密钥；PostgreSQL 从环境变量注入主密钥。客户端凭据不转发到 Provider。响应状态码、错误体和 SSE 字节流保持原样。默认不缓存 LLM 响应。

## 自动识别

优先通过 `anthropic-version` 识别 Anthropic Messages；没有该头时，JSON 顶层 `input` 对应 Responses，顶层 `messages` 对应 Chat。两种信号冲突或缺失时返回 `400`。仅凭 `messages` 无法可靠区分 Chat 与 Anthropic，因此未提供 Anthropic 标识的 Messages 请求不能保证被识别；调用方可使用明确入口。

Pingora 的 `upstream_peer` 在请求体过滤器前执行，而 `request_body_filter` 每次只收到一块数据。因此自动入口必须先限量读取请求体、识别并重放，再建立上游连接；不可仅在 `request_body_filter` 中做上游选择。这个处理器作为单独任务实现，须验证同端口接入和请求体重放。明确入口继续使用 Pingora 的透明流式代理。自动入口设可配置请求体上限，超限返回 `413`。参见 [Pingora 请求阶段](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/phase.md)。

## 可观测性

`llmproxy-telemetry` 负责网关和控制台的公共初始化、服务名、OTLP 三类出口与退出刷新；两侧各自的 `observability` 模块集中记录业务事件。统一通过 `OTEL_SERVICE_NAME` 配置应用服务名，固定的 `component` 字段区分控制台与网关，详见[统一可观测性](10-shared-telemetry.md)。

统一使用 OpenTelemetry：每次请求一个 trace span，结构化日志携带 route/protocol/upstream/status/latency，指标至少包含请求量、失败量和耗时直方图。只用固定的低基数字段做指标标签；不记录完整提示词、响应体、API key 或任意模型名。默认输出本地 JSON 日志。设置 OTLP 环境变量后经 OTLP/HTTP 导出 traces、logs、metrics。OpenObserve 的服务端地址为 `https://<host>/api/<org>`，三类信号分别写到 `/v1/traces`、`/v1/logs`、`/v1/metrics`；认证头由运行环境注入。参见 [OpenObserve OTLP 文档](https://openobserve.ai/docs/ingestion/logs/otlp/)。

类 Nginx/Envoy 的逐请求访问日志采用独立事件和过滤规则，详见[访问日志设计](04-access-logging.md)。

## 安全和错误行为

- 路由以方法和完整路径匹配，未知路径 `404`，不允许的方法 `405`。
- 请求头重写 `Host` 和 Provider 凭据，过滤逐跳头；保留必要的协议头。
- 连接超时与响应读取超时分别配置，SSE 不做全量缓冲或自动压缩。
- `POST` 请求在请求体已发出或响应已开始后不重试，避免重复生成和计费。
- 配置在启动和热更新时校验；显式 PostgreSQL 缺少主密钥或迁移、所选数据库连接失败、SQLite 配套密钥缺失或主密钥错误均阻止启动。运行中加载失败保留上一份有效快照。
- 数据库允许空 Provider 配置，未绑定 Provider 的协议返回 `503`。

## Topcoat 控制台与后续演进

管理页面与网关运行于同一进程、同一端口，控制台挂载 `/ui` 并限制回环客户端访问，提供 Provider 新增、编辑、启停、删除及当前协议绑定。采用 PostgreSQL 持久化、Toasty ORM、AES-256-GCM 凭据加密和乐观版本检查；本机表单使用 Host/Origin 校验和 CSRF token。

登录权限、操作审计、健康检查和聚合指标视图是后续任务。需要多实例配置快速同步时，可在当前轮询基础上增加通知；数据库继续作为配置来源。Topcoat 项目资料见[官方仓库](https://github.com/tokio-rs/topcoat)。
