# 控制台 Shard connections 实施任务

日期：2026-10-08。需求、约束和传输设计见[设计文档 32](32-console-shard-connections-design.md)。

状态：`[x]` 已实现并通过对应核心验收，`[ ]` 尚待专项验收。连接试验先于业务迁移完成；不把未执行的资源压测记为通过。

## 实施顺序与交付

| 任务 | 依赖 | 交付与验收 |
| --- | --- | --- |
| WS0 文档与范围 | 无 | 固定单进程单端口、桥接缺口、迁移顺序和验收证据 |
| WS1 升级桥接试验 | WS0 | 同端口真实 `101`，双向交付、预读字节和慢消费者验证；确定 Pingora 公共接口可实现 |
| WS2 生产桥接与访问保护 | WS1 | 握手分流、Console 共用路由和状态、有界缓冲、关闭清理与受控错误；HTTP 兼容 |
| WS3 重连与 HTTP 路径 | WS2 | 退避重连不重复写操作；明确并验证不可用环境中的 HTTP 使用路径 |
| WS4 聊天实时 shard | WS3 | 消息、停止控件、用量及会话活跃状态共用连接；会话隔离与生成状态正确 |
| WS5 嵌套工作区 | WS4 | 模型候选、草稿、价格及窗口独立更新；评估 Provider 与日历收益 |
| WS6 容量与生命周期回归 | WS4、WS5 | 64 render 边界、取消、慢消费者、连接重建、退出和在途 SSE 回归 |
| WS7 完整验收与文档收口 | WS6 | 浏览器和完整工作区验证通过，同步使用范围、回退操作和运行限制 |

## WS0 文档与范围

- [x] 记录当前 HTTP 桥接的 `OnUpgrade` 和握手头缺口。
- [x] 确定有界进程内 Hyper 桥接为首选候选，并把可行性验证作为前置任务。
- [x] 区分框架重连与 HTTP 回退，列出业务迁移及验收标准。
- [x] 在 README、总任务及统一服务文档建立入口。

## WS1 升级桥接试验

涉及位置：`llmproxy-gateway/src/console.rs`，独立测试夹具；必要时新增小型传输模块。

- [x] 用隔离控制台和一个最小 connected shard，在 Pingora 唯一监听端口完成 Topcoat runtime 握手。
- [x] 通过有界内存流与 Hyper `with_upgrades` 产生真实 `OnUpgrade`，验证共享 Console 路由和状态的调用方式。
- [x] 验证 Pingora 公共 Session 接口的读写调度，服务端主动推送无需等客户端再次发数据。
- [x] 验证握手后立即发送的客户端帧、请求头后预读字节、双向分片、Ping/Pong、Close 与 EOF。
- [x] 验证慢消费者时内存和队列有界；试验进程未创建第二个 TCP listener。
- [x] 固定已验证的模块边界、依赖及缓冲策略；不可行时记录接口限制并修订 WS1，后续保持待实现。

完成条件：真实 WebSocket 客户端与 connected shard 双向交互通过，关闭后任务释放；仅编译成功或仅返回 `101` 不算完成。

## WS2 生产桥接与访问保护

涉及位置：网关 console 传输模块、`llmproxy-console/src/lib.rs`、现有 protect middleware。

- [x] 普通 HTTP 与 Topcoat runtime upgrade 分流；避免对握手使用普通请求体缓冲流程。
- [x] 合法 `101` 保留必要握手头；拒绝响应按 HTTP 返回；普通响应沿用 Pingora 正文分帧。
- [x] 共用 `Console::handle` 和 AppState，不构建第二套 Router、不监听内部 TCP 端口。
- [x] 保留客户端回环校验，验证 Host、Origin、子协议和 HTTP/1.1 边界；非 runtime upgrade 不被任意转接。
- [x] 验证 WebSocket 中的 render 请求继续经过现有路由与保护，不能访问 `/v1/*` 或扩大控制台权限；业务 procedure 保留 CSRF 校验。
- [x] 建立有界桥接、EOF/Close/错误清理和服务退出处理；升级连接禁用 HTTP keepalive 复用。
- [x] 记录受控握手/连接失败，禁止记录凭据、完整帧正文或聊天内容。

完成条件：正常连接、非法握手、跨站和非回环拒绝用例通过；HTTP CRUD、字体、静态资源、procedure 与原有流式 shard 回归通过。

## WS3 重连与 HTTP 路径

- [x] 验证框架断线退避重连与 render 重启，当前信号和会话恢复后不被旧输出覆盖。
- [x] 断线、重连和页面刷新不重新调用 begin/send/save 等写操作。
- [x] 保留未请求 connection 的 HTTP shard 更新和首屏快照路径。
- [x] 明确无升级能力或持续被环境阻止时的判定、传输选择时机和 HTTP 使用方式。
- [x] 用真实浏览器验证选定的 HTTP 回退行为；框架不支持自动切换时，记录限制和明确的 HTTP 模式/回滚步骤，不能声称自动回退已实现。
- [x] 验证访问保护拒绝和非法握手不会触发绕过校验的降级。

完成条件：正常重连及 WebSocket 不可用场景都有可复现的操作与证据；HTTP 回退状态须准确区分自动恢复、明确模式和回滚。

## WS4 聊天实时 shard

涉及位置：`llmproxy-console/src/app/ui/chat.rs` 及聊天室订阅应用服务。

- [x] 对消息、停止按钮、会话活跃状态和用量 shard 请求 connection，共用框架运行时 socket。
- [x] 调整 HTTP 首屏与 connected render 的分支，首屏及时返回，实时广播在连接中继续输出。
- [x] 会话列表、协议和思考选择器采用目标 shard 更新；区分显示依赖与不应触发父渲染的读取。
- [x] 对话切换、分页、归档和删除正确取消旧 render，释放广播订阅，不覆盖新会话状态。
- [x] UI render 停止与模型请求取消分别处理；生成中离开页面再返回仍恢复原轮，点击停止仍调用既有取消入口。
- [x] 验证流式/非流式生成、推理、工具、保存错误、部分 usage 和重试保存的现有行为。

完成条件：单文档只有一个运行时 WebSocket，增量文字在终态前显示，各实时组件状态一致，断线不重复发送模型请求。

## WS5 嵌套工作区

涉及位置：models、models/editor、models/pricing、providers 和 holidays 的 shard。

- [x] 迁移模型工作区、候选列表和编辑草稿，验证子更新不重渲染整个工作区。
- [x] 迁移价格规则、矩阵和峰时窗口，保留 Records 数据及数值精度，保存和预览仍通过服务端校验。
- [x] 添加验收用 render 计数，区分子更新与业务确实需要的列表刷新；避免生产环境正文日志。
- [x] 验证弹窗、选项、筛选、分页、草稿和一次性通知不因父子更新丢失或重复。
- [x] 测量 Provider 列表与节假日工作区的刷新行为；有收益则迁移，无收益则记录保留 HTTP 的理由。

完成条件：子 shard 更新时父工作区及无关 shard 的 render 次数不增加；关键编辑交互浏览器回归通过。

## WS6 容量与生命周期回归

- [x] 测试真实占用的 64 个同时 render、额外 run 的 `429`、停止或完成后恢复容量；拒绝不影响已有 render。
- [ ] 对分页会话、弹窗开关和大量重复导航执行持续压力试验，量化 render 与广播 receiver 数，验证长期无泄漏。
- [x] 客户端 EOF/Close、停机和大快照慢读场景验证通过；桥接内存流与两个 channel 容量有界。
- [ ] 专项测量持续慢读时的 RSS/任务数，补充截断帧故障注入与恢复后资源计数。
- [x] 浏览器前进/后退、快速导航、生成中离开再返回保持文档导航语义，旧页面输出不会覆盖目标页。
- [x] 多标签页连接独立；不同会话的消息、停止和用量相互隔离。
- [x] 同端口活动 UI 连接与 `/v1/*` 在途 SSE 共存测试通过；`/agents/v1/*`、`/internal/subscriptions/*` 的既有订阅代理回归通过。

完成条件：自动化集成与浏览器证据覆盖上述场景，连接关闭后没有残留 render、任务或订阅。

## WS7 完整验收与文档收口

- [x] 使用隔离 SQLite 和模拟 Provider 完成自动化及浏览器核心验收。没有新增 schema 或持久化协议；PostgreSQL 沿用既有环境测试约定，环境相关忽略项如实记录。
- [x] 执行 `bash scripts/fmt.sh --check`、`cargo check --workspace --all-targets --offline` 和 `cargo test --workspace --offline`。
- [x] 记录 listener、WebSocket、文档 loader、render 次数及资源释放证据，说明已测范围和显式忽略项。
- [x] 更新统一服务和本地运行文档，说明已启用组件、连接限制、重连和实际 HTTP 回退方式。
- [x] 有验收证据后再将对应任务改为已完成；性能结论以测量为依据。

## 当前交付记录

2026-10-08：WS0 至 WS5 的接入与核心验收完成；WS6 容量、EOF、停机、多标签页隔离与同端口 SSE 核心回归通过，持续资源压测仍按上方未勾选项跟进。WS7 已同步运行文档并完成核心验证，不声称专项压测已完成。

- 新增 `console_websocket` 五项集成测试：真实 101、握手参数/Host/Origin、run 路由/头/CSRF、2 MiB body 上限、HTTP 模式、70 KB 分片与预读字节、Ping/Pong/Close、停机关闭空闲连接、64 个实际忙碌的聊天 render、429、替换/停止释放容量、EOF 重建、大快照慢读和在途 SSE 共存。
- 浏览器：聊天页六个首批 shard 使用同一条 socket；生成期间增量输出；离页/后退 loader 身份不变，返回后停止原轮；强制关闭连接后重连只恢复 render 请求；价格编辑状态保留。两个标签页各一条活动 socket，停止 A 后 B 仍生成，消息与用量按会话隔离。
- 子 render 计数：添加价格规则只请求规则与矩阵两个子 shard，父工作区增加 0 次；套用峰时预设只请求窗口 shard，父工作区增加 0 次，保留 `0.12345678` 单价；均没有额外 socket 或 JavaScript 错误。
- HTTP 模式浏览器：零 WebSocket，聊天在终态前显示文字，停止与用量正常。Provider 查询只请求列表 shard；日历翻页请求一个 procedure 与一个工作区 shard，因此两者保留 HTTP。
- `lsof` 对隔离网关进程验证只有一个 TCP listener；Hyper 桥接无内部 TCP listener。浏览器故障注入只关闭本次测试 socket，临时网络模拟已恢复。
- 完整工作区命令：`bash scripts/fmt.sh --check`、`cargo check --workspace --all-targets --offline`、`cargo test --workspace --offline`；另执行受影响 console/gateway 的全目标 Clippy（`-D warnings`）。最终计数见下方验证记录。

最终验证记录：格式检查、全工作区全目标编译、受影响 crate 的全目标 Clippy 和 `git diff --check` 均通过；完整工作区测试 **377 通过、0 失败、12 忽略**。忽略项沿用既有手动或环境相关测试约定。隔离浏览器验收进程已退出，测试端口无残留 listener。
