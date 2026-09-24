# llmproxy

基于 Pingora 的 LLM Provider 代理。项目目前处于基础架构阶段。

- [原始需求](docs/00-original-requirements.md)
- [架构设计](docs/01-architecture.md)
- [实施任务](docs/02-tasks.md)
- [本地运行与可观测性](docs/03-development.md)
- [访问日志设计](docs/04-access-logging.md)

当前 workspace 包含不依赖 HTTP 框架的 `llmproxy-core` 和 Pingora 网关 `llmproxy-gateway`。Topcoat 管理控制台在后续阶段加入。
