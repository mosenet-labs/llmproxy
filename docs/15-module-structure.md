# 模块职责整理

## 范围与原则

现有 crate 边界保持不变。本轮只移动代码并调整可见性、导入与路由注册，不改变数据库 schema、HTTP 路径、请求和响应格式、Topcoat 交互或 Pingora 阶段顺序。以当前工作树（含 Chat 思考内容展示）为行为基线。

Topcoat 的 `#[page]` 仍留在原来的 `app::ui::{chat,models,providers}` 模块，以保持页面路径。显式路径的 shard 和 procedure 移动后须保持注解中的 URL，并同步 `Console::connect` 的注册。Pingora 的 `ProxyHttp` 实现保持在一个模块，便于按阶段连续阅读。

## 目标结构

| 模块 | 职责 |
| --- | --- |
| `console::app::chat_sessions` | 页面内会话、消息状态及订阅通知；`AppState` 不再依赖 UI 模块的类型。 |
| `console::app::model_catalog` | Provider 上游 `/models` 请求与候选 ID 解析，供 Providers 和 Models 页面共用。 |
| `console::app::ui::chat` | 页面、Topcoat shard/procedure 及思考内容展示；流式协议解析继续留在 `chat_stream`。 |
| `console::app::ui::models::{editor,actions}` | 编辑器与动态表单、保存/删除/探测操作；`models.rs` 保留页面和列表入口。 |
| `console::app::ui::providers::{editor,actions}` | 编辑表单及写操作；`providers.rs` 保留页面和列表入口。 |
| `store::{providers,models,migrations}` | Provider CRUD、模型映射 CRUD、迁移集；`store.rs` 保留连接、事务和共享校验。 |

拆分按已有职责边界，不按行数机械拆分。`llmproxy-probe` 目前只有单一探测流程；`gateway::proxy` 的阶段实现需要连贯阅读，本轮不拆。

## 实施任务与验收

- [x] R1：抽出 Chat 会话状态。检查切换模型/协议后的新会话、历史会话、流式思考与最终回答状态。
- [x] R2：抽出共用模型目录查询。检查 Provider 表单预览与 Models 候选加载、保存复核的结果一致。
- [x] R3：拆分 Models 与 Providers 的编辑器和操作处理，保持页面函数位置、Topcoat shard/procedure URL 与注册；检查 CRUD、筛选、分页、探测及通知。
- [x] R4：按 Provider、模型映射、迁移拆分 Store 的实现，公共 API 不变；检查 SQLite 和 PostgreSQL 迁移、CRUD、热更新。
- [x] R5：运行格式检查、workspace 测试和差异检查；确认无数据迁移和外部接口变化。

各步骤完成后在[实施任务](02-tasks.md#r模块职责整理)标记，并记录最终验证结果。

## 验证结果

- `cargo fmt --check`、`cargo check --workspace`、`cargo test --workspace` 与 `git diff --check` 均通过；工作区测试共 63 项。
- 使用本地 PostgreSQL 的独立临时 schema 运行 `cargo test -p llmproxy-store`，13 项通过，覆盖 PostgreSQL 升级、Provider 生命周期和并发操作；SQLite 测试也通过。
- Console 集成测试覆盖 Providers、Models、Chat 页面及模型相关 Topcoat 路由。此次没有数据库 schema 或外部接口变更。
