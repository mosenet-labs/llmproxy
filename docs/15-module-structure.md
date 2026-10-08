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

## Toasty 0.11.0 存储优化

2026-10-08：Store、Gateway 和锁文件中的全部 Toasty crate 统一升级至 0.11.0。依据[官方发布说明](https://github.com/tokio-rs/toasty/releases/tag/toasty-v0.11.0)逐项核对实际适用范围；公共存储接口、表结构、迁移文件和加密格式保持原有约定。

| 特性或兼容性项 | 本项目的处理 |
| --- | --- |
| JSON setter 接受借用值 | 模型思考、价格条件、价格时间表和订阅模型声明改用 `Json<T>`；思考、条件和节点模型数组直接借用写入，删除重复手动序列化/反序列化。字段显式指定 `text`，可选字段使用 `Option<Json<T>>`，继续区分 SQL NULL 与 JSON `null`。 |
| 类型化字段保留目标类型 | 多处唯一性与引用检查仅投影 ID；Provider 与路由候选的协议校验仅投影所需 ID/布尔列；旧价格回填仅投影相关价格列。编译与双后端读写覆盖这些投影。 |
| 关系预加载、嵌入关系的模型值引用 | 当前 schema 使用普通整数外键。0.11.0 对此形状的关联 include 在 SQLite 回归中触发 SQL serializer 的 `ExprCast { from: None, ty: I64 }` panic；发布版中不启用该路径。关联记录按去重后的 ID 批量读取，在原事务内组装视图，避免逐条查询及该 panic。 |
| 引擎条件执行 | 上游引擎内部改进，没有需要接入的公开业务 API；通过升级取得修复，不另建业务条件执行层。 |
| 连接数采用 driver 配置 | 保留现有文件 SQLite/PostgreSQL 的连接池上限、超时、WAL、外键 PRAGMA 与写事务模式。应用仍拒绝内存 SQLite；不为 driver 的内存连接限制新增应用入口。 |
| `Deferred` 比较/Hash、新类型 `inner`、Vec 新类型 | 当前持久化模型没有相应字段或缓存键，不引入包装类型或缓存。 |
| 枚举字段、变体 include、NOT、`path`/`from_path` 修复 | 当前模型没有这些查询形状；协议等原有 TEXT 值继续按既有解析规则校验，不改成 PostgreSQL native enum。 |
| `jiff::tz::TimeZone` 字段 | 时间表使用现有经校验的 JSON 表示，不新增列或改变支持的时区规则。 |
| PostgreSQL unique Vec | 节点模型声明需要维持 JSON TEXT 和 SQLite 兼容，不改成 PostgreSQL 数组约束。 |
| MariaDB、Turso HTTP/allocator、MySQL 批量 ID 限制 | 应用后端仍为 SQLite/PostgreSQL，不新增驱动或部署方式。数据库生成的 ID 由现有逐条创建与事务返回，不推断连续 ID。 |

模型列表批量读取其 Provider；路由列表与热加载批量读取候选和模型，再按原候选顺序组装。热加载的模型查询只执行一次，直接模型名称仍按原读取顺序加入。价格列表一次读取当前计划、一次读取相关规则，规则继续按 ID 排序。唯一性和引用检查下推筛选并限制为首个 ID；替换候选与年度节假日使用事务内批量 DELETE，有版本字段的实体继续使用实例更新/删除及原有冲突检查。

`tests/orm/mod.rs` 随 SQLite/PostgreSQL 生命周期测试执行，以真实 driver 的 `toasty::query` 事件统计 SELECT，按测量 span 隔离并发测试；不记录参数值。记录从 1 条增加到 10 条时，模型列表 **5 → 5**、路由列表 **7 → 7**、热加载 **7 → 7**、价格列表 **5 → 5**，包含既有路由绑定与主密钥校验。空集合跳过关联 ID 查询。结果测试覆盖关联归属、候选顺序、禁用候选回退、思考配置、八位小数价格、失败导入回滚、旧时间表默认值、SQL NULL、损坏 JSON 拒绝读取及迁移回填与无关 JSON 的隔离。

PostgreSQL 验证使用新建的临时测试数据库，各测试再创建独立 schema；结束后删除测试数据库，不修改开发数据库。

最终验证：修改范围的 `topcoat fmt --rustfmt --check crates/llmproxy-store`、`cargo check --workspace --all-targets --offline`、`cargo clippy --workspace --all-targets --offline -- -D warnings` 与 `git diff --check` 通过。完整 `cargo test --workspace --offline` 为 **378 通过、0 失败、12 忽略**，忽略项沿用既有手动/环境约定。另在隔离 PostgreSQL 数据库执行 Store 全部测试（包含显式忽略的 PostgreSQL 节点测试），**28 通过、0 失败、0 忽略**；同时验证 SQLite 和 PostgreSQL 的查询计数及旧数据兼容。
