# llmproxy

基于 Pingora 的 LLM Provider 代理。一个进程在同一端口提供三个原生协议入口和 Topcoat Provider 控制台；网关从数据库读取当前 Provider，支持 HTTP/SSE 透传。`/v1/auto` 的协议自动识别仍待实现。

## 快速开始

准备项目相邻目录的 [`topcoat-ant-design`](https://github.com/mosenet-labs/topcoat-ant-design) 组件库（包含项目使用的中文组件和受控 Dialog），在项目根目录运行：

```sh
bash scripts/dev.sh up
```

默认监听 `127.0.0.1:3200`。打开 `http://127.0.0.1:3200/ui` 新增 Provider，并设为当前服务；对应协议的新请求约一秒后使用该配置。

缺省使用 `./data/llmproxy.sqlite3`，首次启动自动迁移并生成相邻的 `.key` 文件；备份时须保存这两个文件。已有 `.env` 若设置 `LLMPROXY_DATABASE_URL`，将优先使用该地址。使用 PostgreSQL 时，还须设置 `LLMPROXY_MASTER_KEY` 并执行 `bash scripts/dev.sh migrate`。运行方式和遥测配置见[本地运行](docs/03-development.md)。

| 入口 | 状态 |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI Chat 透传 |
| `POST /v1/responses` | OpenAI Responses 透传 |
| `POST /v1/messages` | Anthropic Messages 透传 |
| `POST /v1/auto` | 待实现，目前返回 `501` |
| `/ui` | Provider 管理与路由概览 |

## 工程与文档

`llmproxy-core` 定义协议与校验；`llmproxy-store` 封装 Toasty、迁移、Provider 和凭据；`llmproxy-gateway` 承载 Pingora 与统一入口；`llmproxy-console` 提供 Topcoat 页面；`llmproxy-telemetry` 统一 traces、logs、metrics。数据库 SQL 迁移按后端分开维护：[PostgreSQL](crates/llmproxy-store/migrations/postgresql/) · [SQLite](crates/llmproxy-store/migrations/sqlite/)。

| 阅读方向 | 文档 |
| --- | --- |
| 需求与计划 | [原始需求](docs/00-original-requirements.md) · [架构设计](docs/01-architecture.md) · [实施任务](docs/02-tasks.md) |
| 启动与部署 | [本地运行与遥测](docs/03-development.md) · [单进程、单端口与 `/ui`](docs/11-unified-service.md) · [SQLite 与 PostgreSQL](docs/12-sqlite-compatibility-plan.md) |
| 代理实现 | [明确入口联调](docs/05-explicit-routes-validation.md) · [Pingora 请求阶段](docs/06-pingora-request-lifecycle.md) |
| Provider 控制台 | [Provider 管理](docs/07-provider-console.md) · [异步操作与一次性通知](docs/09-console-async-actions.md) |
| 可观测性 | [访问日志](docs/04-access-logging.md) · [网关连接诊断](docs/08-gateway-observability.md) · [统一遥测](docs/10-shared-telemetry.md) |
