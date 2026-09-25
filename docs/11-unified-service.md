# 单进程、单端口与 /ui 控制台

## 已确认需求

用户确认 console 与 gateway 集成，使用一个进程、一个 TCP 监听端口；控制台前缀为 `/ui`，代理入口保留 `/v1/*`。统一 `service.name=llmproxy` 和 OpenObserve 日志流，通过 `component=console/gateway` 区分模块。遥测配置见[统一可观测性](10-shared-telemetry.md)。

## 实现分工

- `llmproxy-gateway` 提供统一 `llmproxy` 二进制，加载 `.env`、初始化一次 telemetry、准备数据库与控制台，启动 Pingora。默认地址为 `127.0.0.1:3200`。
- `llmproxy-console` 改为库，构建 Topcoat Router，不再单独监听 TCP 或初始化 telemetry。
- Pingora `request_filter` 严格匹配 `/ui` 或 `/ui/` 子路径，在进程内调用 Topcoat `Router::handle`；不占用第二个端口。其他路径继续走现有代理逻辑。
- 控制台表单体保持 32 KiB 限制；桥接层限量读取请求体，响应按帧发送。代理和 SSE 不经过控制台的缓冲逻辑。
- 控制台数据库 runtime 在统一服务运行期间保持存活；Pingora 停止后释放资源，再刷新三类遥测。
- 所选数据库是唯一的 Provider 来源；SQLite 和 PostgreSQL 均在启动时执行待应用的迁移，显式 PostgreSQL 仍须配置主密钥。控制台与代理读取同一数据库，代理请求使用内存快照。详见[SQLite 兼容方案](12-sqlite-compatibility-plan.md)。
- 控制台保留本机访问、Host/Origin/CSRF 保护；即使代理绑定非回环地址，`/ui` 仍只接收回环客户端。管理登录与权限属于后续任务。

## /ui 路径

| 外部路径 | 用途 |
| --- | --- |
| `/ui` | Provider 管理 |
| `/ui/routes` | 路由概览 |
| `/ui/providers/*` | 表单与兼容 POST 接口 |
| `/ui/assets/*` | CSS 与 Topcoat runtime |
| `/ui/_topcoat/*` | 字体、原生 procedure/shard/page 重渲染 |
| `/v1/*` | 现有明确代理入口与自动入口占位 |

Topcoat 0.8.1 的 runtime 固定生成 `/_topcoat` URL，尚无应用挂载前缀配置。因此构建时只适配框架 bundle 的固定端点前缀，并更新资源 hash；服务端仅将 `/ui/_topcoat/` 还原为框架内部路径。字体通过原生 FontResolver 添加前缀。页面路由和链接直接声明 `/ui`，不做 HTML 内容替换，不改业务交互。构建与 HTTP/浏览器验收覆盖这项兼容逻辑。

## 配置收敛

```dotenv
LLMPROXY_LISTEN=127.0.0.1:3200
# 以下两项仅在使用 PostgreSQL 时设置；默认 SQLite 可省略
# LLMPROXY_DATABASE_URL=postgresql://<用户>:<密码>@127.0.0.1:5432/llmproxy_dev
# LLMPROXY_MASTER_KEY=<Base64 编码的 32 字节主密钥>
OTEL_SERVICE_NAME=llmproxy
OTEL_EXPORTER_OTLP_HEADERS="authorization=Basic%20<现有凭据>,stream-name=llmproxy"
```

移除 `LLMPROXY_CONSOLE_PORT`、`LLMPROXY_GATEWAY_SERVICE_NAME`、`LLMPROXY_CONSOLE_SERVICE_NAME`，不增加独立日志流变量。OTLP 地址、认证和超时继续沿用现有配置。组件字段进入日志、span 与指标的固定标签，不创建第二组 exporter。控制台请求不再重复记录为网关请求。

## 任务与验收

- [x] U1：统一入口、控制台库化与 Pingora/Topcoat 桥接。
- [x] U2：页面、资源、字体与原生异步端点全部迁到 `/ui`。
- [x] U3：收敛服务名、日志流、component 字段、本地配置和启动脚本。
- [x] U4：同端口 HTTP 验证 CRUD、保护、资源、代理生效、SSE 与路由边界。
- [x] U5：浏览器验证原生异步操作，完成 workspace 检查与文档更新。

## 验收记录

- `cargo fmt --all --check`、`cargo check --workspace --offline`、`cargo test --workspace --offline` 通过，完整测试 36 项；PostgreSQL 场景已启用，在独立 schema 执行并清理。
- 控制台 HTTP 验收覆盖资源、字体、原生 procedure/shard/page 重渲染、CRUD、Host/Origin/CSRF 和体积限制。同端口创建并激活模拟上游后，实际代理响应与替换凭据正确。
- `/uix`、`/ui-other` 与未带 `/ui` 前缀的控制台端点返回 404。已有 SSE 期间可访问 `/ui/routes`，数据库更新保留在途请求的快照。
- 本地 OTLP 接收端解码 traces/logs/metrics，验证 `service.name=llmproxy`、共享 `stream-name`、日志与 trace 关联和正常退出刷新。控制台请求不重复输出网关完成事件。
- 浏览器验证新建弹窗、异步创建、设为当前服务、路由概览与刷新；通知包含名称，刷新不重播，无浏览器错误。独立测试进程只有一个 TCP 监听端口，验证后进程、日志与 schema 已清理。
- 忽略提交的本地 `.env` 已更新监听地址、统一服务名与日志流；保留原 OTLP 地址和认证信息，验收未向真实 OpenObserve 发送测试数据。

## 本地运行

在项目根目录运行 `cargo run` 或 `bash scripts/dev.sh up`，进程会先迁移所选数据库，再加载 Provider 并监听端口。PostgreSQL 须设置固定的 `LLMPROXY_MASTER_KEY`；独立的 `bash scripts/dev.sh migrate` 仍可使用。访问 `http://127.0.0.1:3200/ui`，三个代理入口使用同一主机与端口。更新后需停止旧进程，再启动统一服务。

技术依据：[Topcoat Router::handle](https://docs.rs/topcoat/0.8.1/topcoat/router/struct.Router.html#method.handle)；Pingora 0.9.0 `ProxyHttp::request_filter`、Session 读写接口；本地 Topcoat runtime/font 源码。
