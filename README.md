# llmproxy

基于 Pingora 的 LLM Provider 代理。已实现三个明确入口的代理，并通过本地 HTTP/SSE 自动化联调。

- [原始需求](docs/00-original-requirements.md)
- [架构设计](docs/01-architecture.md)
- [实施任务](docs/02-tasks.md)
- [本地运行与可观测性](docs/03-development.md)
- [访问日志设计](docs/04-access-logging.md)
- [三个明确入口的阶段分工与联调验收](docs/05-explicit-routes-validation.md)
- [Pingora 请求生命周期与阶段串联](docs/06-pingora-request-lifecycle.md)

当前 workspace 包含不依赖 HTTP 框架的 `llmproxy-core` 和 Pingora 网关 `llmproxy-gateway`。Topcoat 管理控制台在后续阶段加入。
