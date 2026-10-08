# llmproxy

基于 Pingora 的 LLM Provider 代理。一个进程在同一端口提供四种原生协议入口，以及 Topcoat 构建的 Providers、Models、Model Routes 控制台。网关按模型标识或显式路由名称选择上游模型与 Provider，支持 HTTP/SSE 转发。`/v1/auto` 的协议自动识别仍待实现。

## 快速开始

控制台使用 Topcoat 0.10.0 与crates.io 发布包 [`topcoat-ant-design`](https://github.com/mosenet-labs/topcoat-ant-design) 0.3.0，具体版本由 Cargo.lock 锁定，无需准备相邻目录的组件库。在项目根目录运行：

```sh
bash scripts/dev.sh up
```

默认监听 `127.0.0.1:3200`。打开 `http://127.0.0.1:3200/ui/providers` 新增并启用 Provider，配置其支持的协议、上游路径和模型探测接口（默认 `/models`）；然后到 `/ui/models` 添加具体模型并设置至少一个协议。模型标识可直接用于客户端请求。到 `/ui/routes` 可创建新的对外模型名，添加不同 Provider 的候选模型并调整顺序。OpenAI 和 Anthropic 请求在请求体顶层 `model` 字段填写模型标识或路由名称，网关将其改写成所选上游模型 ID；Gemini 请求在路径中填写。控制台保存后，新请求约一秒内使用新配置；没有模型和路由时服务可启动，但代理请求不会自动选择 Provider。

Gemini 原生接入：在 Providers 中选择 Gemini，上游地址填写 `https://generativelanguage.googleapis.com`，Gemini 上游模型路径前缀填写 `/v1beta/models`，模型列表路径填写 `/v1beta/models`，API Key 使用 Google AI Studio 的 Gemini API Key。保存后在 Models 中关联 Gemini 模型。客户端路径中的 `{model}` 使用模型标识或 Model Route 名称；请求体和响应保持 Gemini 原生格式。

缺省使用 `./data/llmproxy.sqlite3`，首次启动生成相邻的 `.key` 文件；备份时须保存这两个文件。已有 `.env` 若设置 `LLMPROXY_DATABASE_URL`，将优先使用该地址。SQLite 和 PostgreSQL 均在启动时检查并执行待应用的迁移；使用 PostgreSQL 时还须设置固定的 `LLMPROXY_MASTER_KEY`。运行方式和遥测配置见[本地运行](docs/03-development.md)。

| 入口 | 状态 |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI Chat 透传 |
| `POST /v1/responses` | OpenAI Responses 透传 |
| `POST /v1/messages` | Anthropic Messages 透传 |
| `POST /v1beta/models/{model}:generateContent` | Gemini 原生普通响应透传 |
| `POST /v1beta/models/{model}:streamGenerateContent` | Gemini 原生 SSE 透传 |
| `POST /v1/auto` | 待实现，目前返回 `501` |
| `/ui/providers`、`/ui/models`、`/ui/routes` | Provider、具体模型与 Model Routes 管理；`/ui` 跳转至 Providers |

## 工程与文档

`llmproxy-core` 定义协议与校验；`llmproxy-store` 封装 Toasty、迁移、Provider、模型映射和凭据；`llmproxy-gateway` 承载 Pingora 与统一入口；`llmproxy-console` 提供 Topcoat 页面；`llmproxy-telemetry` 统一 traces、logs、metrics。数据库 SQL 迁移按后端分开维护：[PostgreSQL](crates/llmproxy-store/migrations/postgresql/) · [SQLite](crates/llmproxy-store/migrations/sqlite/)。

| 阅读方向 | 文档 |
| --- | --- |
| 需求与计划 | [原始需求](docs/00-original-requirements.md) · [架构设计](docs/01-architecture.md) · [实施任务](docs/02-tasks.md) |
| 启动与部署 | [本地运行与遥测](docs/03-development.md) · [单进程、单端口与 `/ui`](docs/11-unified-service.md) · [SQLite 与 PostgreSQL](docs/12-sqlite-compatibility-plan.md) |
| 代理实现 | [明确入口联调](docs/05-explicit-routes-validation.md) · [Pingora 请求阶段](docs/06-pingora-request-lifecycle.md) |
| Provider 与模型控制台 | [Provider 管理](docs/07-provider-console.md) · [多协议 Provider 与模型探测](docs/13-provider-interfaces-and-model-discovery.md) · [模型映射与 Models 控制台](docs/14-model-mapping.md) · [Model Routes](docs/18-model-routes.md) · [模型参考价格规则](docs/16-model-pricing.md) · [节假日日历](docs/17-holiday-calendar.md) · [异步操作与一次性通知](docs/09-console-async-actions.md) |
| 可观测性 | [访问日志](docs/04-access-logging.md) · [网关连接诊断](docs/08-gateway-observability.md) · [统一遥测](docs/10-shared-telemetry.md) |
| 控制台实时更新 | [共享 WebSocket 设计](docs/32-console-shard-connections-design.md) · [Shard connections 任务](docs/33-console-shard-connections-tasks.md) |
| 工程维护 | [模块职责整理](docs/15-module-structure.md) |
