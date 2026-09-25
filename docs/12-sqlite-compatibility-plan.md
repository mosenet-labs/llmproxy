# SQLite 兼容与默认数据库

## 目标与范围

Provider 仍只从数据库加载。服务支持 PostgreSQL 和 SQLite；未设置 `LLMPROXY_DATABASE_URL` 时，使用持久化的本地 SQLite 文件。控制台、网关快照和数据库工具选择同一个后端。PostgreSQL 与 SQLite 之间的数据导入、导出或自动迁移均不在本次范围内；切换后看到的是所选数据库自己的 Provider 记录。

S1–S5 已实施。以下配置约定适用于当前程序。

## 配置契约

| 配置 | 预期行为 |
| --- | --- |
| `LLMPROXY_DATABASE_URL` 缺失或空白 | 使用 `sqlite:./data/llmproxy.sqlite3`；相对路径以进程工作目录为准，启动日志输出解析后的绝对文件路径。 |
| `sqlite:<文件路径>` | 使用指定的持久化 SQLite 文件，首次启动创建父目录并应用 SQLite 迁移。 |
| `postgresql://...` 或 `postgres://...` | 保持现有 PostgreSQL 路径和显式迁移流程。 |
| 显式地址格式错误或数据库不可用 | 启动失败；不得悄悄切换到另一种数据库。 |

默认路径使用文件数据库，不使用 `sqlite::memory:`。当前网关快照与控制台各自建立连接，内存数据库无法作为它们共享的持久化 Provider 来源；统一服务入口应明确拒绝此 URL。`scripts/dev.sh` 已切换到项目根目录，因此默认相对路径在本地开发时稳定。部署在其他工作目录时应显式设置绝对 SQLite 路径。现有 `.env` 如果写了 PostgreSQL URL，仍会选择 PostgreSQL；要使用默认 SQLite，须移除该项。

### 主密钥

Provider API Key 继续加密存储。PostgreSQL 保持 `LLMPROXY_MASTER_KEY` 必填。SQLite 优先使用显式的 `LLMPROXY_MASTER_KEY`；没有设置时，从数据库文件旁的 `<数据库文件>.key` 读取 32 字节随机主密钥的 Base64 文本。仅当数据库文件和密钥文件均不存在时，首次启动生成该文件，并使用当前用户可用的私有文件权限；创建应是原子的，不覆盖已有文件。数据库已存在而密钥文件丢失时必须启动失败，不能生成一把新密钥假装恢复。显式主密钥错误时同样报错，不回退到密钥文件。

SQLite 数据库文件与密钥文件都要备份；恢复时需要配对。这里的文件密钥主要解决默认启动的配置成本，不改变现有数据库主密钥校验及凭据不回显的行为。日志不得输出密钥、数据库 URL 中的密码或 Provider API Key。

## 存储层改造

1. `llmproxy-store` 为 Toasty 0.10 启用 `sqlite` feature，在一处解析数据库 URL 并确定后端；网关入口和 `llmproxy-db` 复用该解析结果。Provider 模型、加密、控制台操作和网关快照 API 保持共用。
2. 保留已有 PostgreSQL 迁移内容、编号与校验历史，新增独立的 SQLite 迁移集。SQLite 建表使用兼容的自增主键、长度检查和整数/布尔存储；包括 `providers`、三条 `route_bindings` 和 `store_keys`。Toasty SQLite 驱动每个迁移文件执行一条语句，因此 SQLite 的建表和种子数据分成四个版本。首次 SQLite 启动自动应用版本化迁移，后续启动幂等；`llmproxy-db migrate` 对两种后端使用同一选择规则。PostgreSQL 保持现有显式迁移方式。
3. 把后端差异限制在迁移和事务辅助函数。PostgreSQL 保留 advisory lock、`FOR UPDATE/SHARE`；SQLite 写事务在开始时取得写锁（`BEGIN IMMEDIATE`），读事务提供一致快照，不生成 PostgreSQL 的行锁 SQL。Provider 启停、设为当前、删除、版本冲突和快照读取的业务规则保持一致。
4. SQLite 每次从池中取连接时设置 `foreign_keys=ON` 与 5 秒 `busy_timeout`，连接文件时设置 WAL；测试同时持有两个池连接验证外键约束、等待配置和 WAL。Toasty 0.10 驱动没有连接初始化 hook，因此这些 PRAGMA 集中在存储层的连接辅助函数。SQLite 用于本机单实例部署，多实例共享数据库仍以 PostgreSQL 为主。

Toasty 0.10 提供 [SQLite 驱动](https://docs.rs/toasty/0.10.0/toasty/)；其[数据库连接说明](https://tokio-rs.github.io/toasty/nightly/guide/database-setup.html)使用 `sqlite:<路径>`。SQLite 写锁可通过 Toasty 的[事务模式](https://tokio-rs.github.io/toasty/nightly/guide/transactions.html)表达。外键需[逐连接启用](https://www.sqlite.org/foreignkeys.html)；[WAL](https://www.sqlite.org/wal.html)允许读写并行，但仍只有一个写者。

## 启动、测试与文档

- 网关启动时先解析后端和主密钥；SQLite 完成文件准备与迁移后，再加载初始 Provider 快照并启动 `/ui` 和 `/v1/*`。初始化失败不监听端口。`scripts/dev.sh up` 不依赖本机 PostgreSQL 即可启动默认 SQLite。
- 测试使用临时 SQLite 文件及配套密钥文件，完整跑 Provider CRUD、当前绑定、版本冲突、错误密钥、控制台操作、代理热更新和 SSE；保留现有隔离 PostgreSQL 回归测试。工作区测试在没有 PostgreSQL 服务与测试 URL 的环境中也应执行 SQLite 路径，而不是跳过所有数据库验证。
- 验证缺省地址首次启动、重启后记录和凭据可用、数据库存在但密钥文件缺失时拒绝启动、显式 PostgreSQL 地址连接失败时不回退，以及两种后端的迁移重复执行。
- 更新 `.env.example`、README、运行文档和 `.gitignore`：默认 SQLite 不填写数据库 URL；将默认数据库目录及密钥文件排除版本控制。文档说明当前 `.env` 的 PostgreSQL URL 仍会优先生效。

## 实施顺序与验收

| 步骤 | 交付物 | 验收点 |
| --- | --- | --- |
| S1 | 后端选择、默认路径和 SQLite 主密钥文件 | 缺省配置解析为 SQLite；已有数据库缺密钥时拒绝启动；显式 PostgreSQL 失败不回退。 |
| S2 | SQLite 迁移与 Toasty 连接 | 新文件可初始化；重复迁移无变化；已有 PostgreSQL 数据库不受影响。 |
| S3 | 按后端选择事务和锁 | 双后端 CRUD、绑定切换、并发写入、版本冲突及一致快照通过。 |
| S4 | 统一启动入口与开发脚本 | 无 PostgreSQL 的机器能启动 `/ui` 并通过代理；重启后 Provider 保留。 |
| S5 | 测试与运行文档 | 无 PostgreSQL 时 SQLite 集成测试通过；有 PostgreSQL 时双后端测试及 workspace 检查通过。 |

## 实施与验收记录

- 默认启动创建 `./data/llmproxy.sqlite3` 与 0600 权限的配套 `.key` 文件。重启后控制台读取既有 Provider；数据库存在但密钥文件缺失时拒绝启动。显式 PostgreSQL 连接失败和 `sqlite::memory:` 均不回退。
- SQLite 的 Provider 生命周期、三种协议绑定、版本冲突和并发写入沿用原业务测试；控制台 CRUD、三个明确入口、HTTP/SSE 和遥测集成测试使用临时 SQLite。PostgreSQL 迁移、Provider 生命周期、网关热更新与 SSE 在本地测试库的独立 schema 上回归通过。
- `cargo fmt --all --check`、`cargo check --workspace`、`cargo test --workspace` 作为最终检查；SQLite 路径不要求 PostgreSQL。现有 `.env` 若仍设置 PostgreSQL URL，继续选择 PostgreSQL。
