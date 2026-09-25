# 实施任务

状态：`[x]` 已完成，`[ ]` 待实现。P0 基础架构、P1 三个明确入口的本地联调，以及 P2 Provider 控制台与数据库配置热更新已完成；其余任务保留验收标准。

## P0：基础架构

- [x] 建立 Rust 2024 workspace、`llmproxy-core` 和 `llmproxy-gateway`，采用本次核对的当前稳定 crate 版本并生成锁文件。
- [x] 落地 Provider 定义、四个路由的领域模型、自动识别的纯函数和单元测试。
- [x] 添加数据库环境变量、启动校验和运行文档。
- [x] 接入本地结构化日志与 OTLP traces、logs、metrics 导出基础设施。
- [x] 创建 Pingora 网关入口和三个明确路径的代理骨架。
- [x] 本地模拟上游烟测：三个明确入口均返回 `200`，路径和 Provider 凭据正确改写；`/v1/auto`、未知路径、不支持的方法分别返回 `501`、`404`、`405`。
- [x] 本地 OTLP 接收端烟测：收到 `/v1/traces`、`/v1/logs`、`/v1/metrics` 三类请求。

## P1：代理功能完成

- [x] 联调三个明确入口：Host/凭据/协议头、错误透传、SSE、超时和未知路径。详见[阶段分工与验收矩阵](05-explicit-routes-validation.md)。
  - [x] 落地阶段分工、行为约定与验收矩阵。
  - [x] 实现超时配置、Host/405 修复、故障映射和重试约束。
  - [x] 添加并通过三个入口的实际 HTTP/SSE 自动化集成测试（12 项，含各协议的参数化场景）。
- [ ] 实现 `/v1/auto` 的限量读取、严格协议识别、请求体重放和上游路径改写；验证同端口接入。冲突请求返回 `400`，超限返回 `413`。
- [ ] 明确网关客户端鉴权方案与密钥轮换，并实现鉴权和限流。
- [x] 验证 `POST` 请求的失败处理，确保已发送请求体或响应后的请求不自动重试；当前策略明确关闭连接阶段和转发阶段自动重试。
- [ ] 接入 W3C `traceparent` 上下文提取与上游传播，验证跨服务 trace 关联。
- [ ] 按[访问日志设计](04-access-logging.md)实现独立的逐请求 JSON 访问日志；验证 `RUST_LOG=warn` 时仍输出、错误及本地拒绝路径恰好一条、SSE 在结束时记录，并观测队列丢弃数。
- [ ] 在现有三协议 HTTP/SSE 端到端测试上扩展遥测标签和 OTLP 输出断言。

## P2：Provider 管理与运营能力

详细设计与运行说明见 [Provider 管理控制台与 PostgreSQL](07-provider-console.md)。

- [x] 增加 `llmproxy-store`，使用 Toasty 0.10 / PostgreSQL，提交版本化 SQL 迁移，开发和测试库分离。
- [x] 实现 Provider 新增、查询、编辑、启停、删除，以及每种协议的当前绑定；事务保证切换与解绑一致，版本检查防止陈旧表单覆盖。
- [x] API Key 加密落库，主密钥由环境变量注入，数据库校验记录阻止错误主密钥读写；页面不回显凭据，编辑留空保留原值。
- [x] 增加 Topcoat 控制台模块，复用本地 `topcoat-ant-design` 的表格、表单、状态标签和确认组件，支持搜索、协议/状态筛选和中文反馈。
- [x] Provider 编辑采用 Topcoat Signal 和原生表单交互，扩展组件库的受控 Dialog；按协议显示字段、连接项同行对齐、超时默认折叠。
- [x] 控制台仅接受回环客户端，验证 Host / Origin / CSRF，限制表单体积。
- [x] 网关读取 PostgreSQL 的不可变 Provider 快照，每秒刷新，加载失败保留上一版；请求持有原配置，热更新不打断已有 SSE。
- [x] 提供 `.env.example`、本地启动/迁移脚本及分层和运行文档。
- [x] 验证真实 PG CRUD、迁移重复执行、错误主密钥拒绝、版本冲突与凭据加密。
- [x] 验证真实控制台 HTTP 操作、浏览器页面与 UI 保存后网关转发生效。
- [x] 验证 DB 到网关的激活、切换、密钥更新、禁用解绑、旧 SSE 保持和刷新故障恢复。
- [ ] Provider 健康检查与状态展示。
- [ ] 控制台聚合指标视图。
- [ ] 多用户管理身份验证、权限与操作审计。
- [ ] 主密钥轮换工具及备份恢复流程。

## O：网关可观测性收口

详见[需求、阶段分工与验收约定](08-gateway-observability.md)。

- [x] O1：记录 HttpPeer/tracer 能力边界及集中记录设计。
- [x] O2：统一请求 span、完成日志、指标及运行事件出口。
- [x] O3：补充 DNS、TCP/TLS、连接复用和受控错误诊断。
- [x] O4：验证请求恰好一次记录、SSE、复用不重复握手、错误脱敏与三类遥测关联。
- [x] O5：同步运行文档并通过 workspace 完整验证。

本轮验证：`cargo fmt --all -- --check`、`cargo check --workspace --offline`、`cargo test --workspace --offline` 均通过；完整测试 33 项，包含真实本地 HTTP/SSE 与隔离 PostgreSQL 用例。

## C：控制台异步操作与一次性通知

详见[已确认方案与验收](09-console-async-actions.md)。

- [x] C1：记录重复通知原因、异步方案和任务。
- [x] C2：将保存和列表写操作改为 Topcoat 原生异步过程。
- [x] C3：局部更新列表，一次性通知，保留筛选与页面位置。
- [x] C4：处理业务失败、提交状态与凭据清理。
- [x] C5：完成 HTTP、浏览器回归和 workspace 验证。

本轮验证：`cargo fmt --all -- --check`、`cargo check --workspace --offline`、`cargo test --workspace --offline` 均通过，完整测试 33 项。浏览器验证记录见[异步操作文档](09-console-async-actions.md#验证记录)。

## T：控制台与公共可观测性

详见[统一可观测性设计与配置](10-shared-telemetry.md)。

- [x] T1：提取公共初始化、服务名解析和三类 exporter；网关与控制台统一加载 `.env`。
- [x] T2：控制台集中记录运行、HTTP 请求和 Provider 操作，补齐 span 与指标。
- [x] T3：验证服务名、OTLP 三类载荷、真实 HTTP 日志及脱敏；服务名配置现按 U3 统一。
- [x] T4：同步配置说明并完成 workspace 验证。

本轮验证：格式检查、workspace 编译与 36 项测试通过，包含隔离 PostgreSQL、控制台 HTTP 日志和本地 OTLP 三类载荷验证；未向真实 OpenObserve 发送测试数据。

## U：单进程与 /ui 单端口集成

详细任务与验收见[统一服务](11-unified-service.md)。

- [x] U1：统一入口、控制台库化与进程内请求桥接。
- [x] U2：控制台所有页面、资源与异步接口迁到 `/ui`。
- [x] U3：统一 telemetry 服务名、日志流和 component 字段，更新配置与启动脚本。
- [x] U4：同端口 HTTP/PG、代理、SSE 与路由边界回归。
- [x] U5：浏览器回归、文档更新与 workspace 验证。

本轮验证：格式检查、workspace 编译与 36 项测试全部通过，包含真实 PostgreSQL、同端口 UI 到代理生效及本地 OTLP 接收端；浏览器验证创建、激活、导航和刷新无重复通知，并确认测试进程只监听一个 TCP 端口。详见[验收记录](11-unified-service.md#验收记录)。

## V：数据库唯一配置来源

见[原始需求](00-original-requirements.md)。

- [x] 移除文件配置、环境变量 Provider 凭据解析、可选控制台分支及相关示例；启动统一要求数据库 URL 和主密钥。
- [x] 保留上游参数校验，供数据库写入和快照加载共用。
- [x] 明确入口及可观测性联调改用隔离 PostgreSQL schema，不再通过文件注入 Provider。
- [x] 验证缺少数据库配置时启动失败、三个协议入口联调及 workspace 测试。

本轮验证：格式检查、workspace 编译和 35 项测试通过，包含隔离 PostgreSQL、三个协议入口的 HTTP/SSE、控制台操作和缺失配置的启动检查。

## S：SQLite 兼容与默认数据库

详细范围、配置约定及验收见[SQLite 兼容计划](12-sqlite-compatibility-plan.md)。不包含 PostgreSQL 与 SQLite 之间的数据迁移。

上文任务保留实施当时的 PostgreSQL 验收记录；以下 S 任务改变了缺省数据库和测试路径。

- [x] S1：统一后端选择，缺省使用持久化 SQLite 文件；SQLite 首次启动生成并保存本地主密钥。
- [x] S2：增加 SQLite 迁移集和自动初始化，保留现有 PostgreSQL 迁移历史。
- [x] S3：按后端处理事务及锁，验证双后端 Provider 管理和快照一致性。
- [x] S4：统一网关、数据库工具和开发脚本的启动行为，更新运行文档。
- [x] S5：增加默认 SQLite 的端到端测试，保留 PostgreSQL 回归并完成 workspace 验证。

## 完整验收

P0 应通过 `cargo fmt --check`、`cargo test --workspace` 和 `cargo check --workspace`。P1 应通过本地模拟上游的实际 HTTP/SSE 请求；P2 应验证管理变更无须重启代理即可生效。
