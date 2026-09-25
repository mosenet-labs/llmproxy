# 原始需求

## 用户原话

> 当前项目是一个空项目，基于当前项目我想使用pingora实现一个简单的llm provider的代理：
> 可以有4个路由：三个路由分配对应openai chat、openai response、anthropic message的路由与一个自动的路由，项目上怎么做分层设计,以及项目架构要怎样?
> 后期会增加一个使用topcoat做UI控制台

> 好的，并我提的原始需求落地到docs下原始需求文档中，并将你分析的写入新的文档，原始需要需要有链接跳转到关联文档，再将你的分析拆分成任务也落地文档，并根据任务文档先在本地创建项目的基础架构，项目依赖都使用最新版本的库，要支持trace logs metrics，上报的服务可以考虑使用 openobserve

## 补充确认

“自动路由”是**自动识别 OpenAI Chat、OpenAI Responses、Anthropic Messages 三种协议**，而不是根据模型自动选择上游。

本轮优先实施：

> 联调三个明确入口：Host/凭据/协议头、错误透传、SSE、超时和未知路径。

> 文档先落地，开始实现

对应的 Pingora 阶段分工、行为约定和验收矩阵见[三个明确入口的联调验收](05-explicit-routes-validation.md)。

## 关联文档

最新确认：单进程、单端口集成控制台与网关；控制台使用 `/ui`，统一服务名和日志流，执行实现。见[统一服务设计与任务](11-unified-service.md)。

> 不用/admin改用/ui 其他的按你的建议来，执行吧

此前确认（服务名划分已由上述单进程方案替代）：控制台补齐 tracing 日志，与网关共享可观测性初始化；一份 `.env` 分别配置两个进程的 `service.name`。当前实现见[控制台与网关统一可观测性](10-shared-telemetry.md)。

补充确认：Provider 写操作采用异步提交、局部更新列表和本次操作通知，避免通过 URL 参数反复弹出成功提示。用户要求“确定后写入文档并拆分任务后进行修复”。详见[异步操作与一次性通知](09-console-async-actions.md)。

补充需求：沿用 Pingora 原生 `HttpPeer`，整合 `PeerOptions::tracer` 与 Rust `tracing`，观测 DNS、TCP 建连、TLS 握手、连接复用和错误。用户进一步要求：

> 还有tracing logs metrics的记录尽量不要太分散，记录当前的需求与分析到文档拆分任务，并开始优化改造

对应的能力边界、集中记录方案和验收任务见[网关可观测性收口](08-gateway-observability.md)。

补充需求：Provider 通过 Topcoat UI 管理，采用 Ant Design 清爽简约后台风格，复用 `topcoat-ant-design` 组件库（缺少通用组件时可扩展）；ORM 使用 Toasty，数据库使用本地 PostgreSQL。连接凭据仅保存于本地环境配置。详见[Provider 控制台设计](07-provider-console.md)。

- [架构设计与技术边界](01-architecture.md)
- [分阶段实施任务与完成状态](02-tasks.md)
- [本地运行和 OpenObserve 配置](03-development.md)
- [访问日志设计](04-access-logging.md)
- [三个明确入口的阶段分工与联调验收](05-explicit-routes-validation.md)
- [Pingora 请求生命周期与阶段串联](06-pingora-request-lifecycle.md)
- [Provider 管理控制台与 PostgreSQL](07-provider-console.md)
- [网关可观测性收口与连接诊断](08-gateway-observability.md)
- [Provider 异步操作与一次性通知](09-console-async-actions.md)
- [控制台与网关统一可观测性](10-shared-telemetry.md)
- [单进程、单端口与 /ui 控制台](11-unified-service.md)
