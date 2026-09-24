# llmproxy

基于 Pingora 的 LLM Provider 代理。已实现三个明确入口的代理，并通过本地 HTTP/SSE 自动化联调。

- [原始需求](docs/00-original-requirements.md)
- [架构设计](docs/01-architecture.md)
- [实施任务](docs/02-tasks.md)
- [本地运行与可观测性](docs/03-development.md)
- [访问日志设计](docs/04-access-logging.md)
- [三个明确入口的阶段分工与联调验收](docs/05-explicit-routes-validation.md)
- [Pingora 请求生命周期与阶段串联](docs/06-pingora-request-lifecycle.md)
- [Provider 管理控制台与 PostgreSQL](docs/07-provider-console.md)

当前 workspace 包含 `llmproxy-core`（领域模型）、`llmproxy-store`（Toasty/PostgreSQL）、`llmproxy-gateway`（Pingora）和 `llmproxy-console`（Topcoat 控制台）。

## 本地启动控制台与网关

将包含中文组件和受控 Dialog 支持的 `topcoat-ant-design` 放在项目相邻目录，创建 PostgreSQL 开发库，并按 `.env.example` 配置本地 `.env`。详细步骤见 [Provider 控制台](docs/07-provider-console.md)。

```sh
bash scripts/dev.sh up
```

控制台：`http://127.0.0.1:3200`；网关：`http://127.0.0.1:8080`。在控制台新增 Provider 并“设为当前”后，对应入口约一秒生效。
