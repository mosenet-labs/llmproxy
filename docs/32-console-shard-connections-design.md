# 控制台共享 WebSocket 与 Shard connections 设计

日期：2026-10-08。

控制台采用 Topcoat v0.10.0 的 Shard connections，让同一浏览器文档中的实时 shard 共用一条 WebSocket，并允许子 shard 独立更新。先接通 Pingora 与 Topcoat 的连接升级，再迁移聊天页，最后迁移有收益的嵌套工作区。

状态：同端口桥接、聊天与模型子 shard 共享连接、显式 HTTP 模式已实现并通过核心验收。逐项证据及剩余专项回归见[任务清单 33](33-console-shard-connections-tasks.md)。

关联：[单进程单端口](11-unified-service.md) · [异步操作](09-console-async-actions.md) · [聊天历史实现](29-conversation-history-implementation.md)。

## 已确认目标

用户确认接入 Shard connections，先落地文档与任务拆分，再开始实施。目标包括：

- 一个 llmproxy 进程、一个 TCP 监听端口，Pingora 继续负责接入；页面、资源和运行时请求保留在 `/ui`。
- 同一文档的 connected pages 和 shards 共用运行时 WebSocket；不同标签页各有自己的文档与连接。
- 聊天消息、生成状态、停止控件、用量和历史会话活跃状态分别更新，减少同时长期占用的 HTTP 请求。
- 模型工作区内的候选、编辑草稿、价格规则、价格矩阵和峰时窗口按自身依赖更新，减少父工作区的重渲染。
- 保留 HTTP 页面、procedure 和 shard 路径；无 JavaScript 时页面及既有普通表单仍可使用，实时交互需要运行时。
- 保留控制台回环客户端限制、Host、Origin、CSRF、请求体限制和服务端输入校验。

模型调用和数据持久化继续使用现有聊天应用服务、Provider HTTP/SSE 与数据库。此次连接负责 UI 渲染和信号同步；停止某个 UI render 不等于取消模型调用。点击“停止生成”仍通过现有业务操作取消对应调用。

## 接入前的缺口

接入前，`llmproxy-console/src/lib.rs` 已注册 `.runtime()`，运行时每连接渲染上限为 64；业务 shard 尚未调用 `connected(cx)`。聊天的消息、停止按钮、会话活跃状态和用量通过 `live!` 在 POST shard 请求中等待广播，并增量输出；GET 首屏只输出快照。

原 `llmproxy-gateway/src/console.rs` 将 Pingora 请求的 method、URI、headers 和 body 转成 Topcoat Request，再按响应帧写回。普通 HTTP 路径有两个升级缺口：

1. Topcoat 的 `WebSocketUpgrade` 从请求 extensions 取得 Hyper `OnUpgrade`。直接构造 Request 不会生成它；需要由 Hyper 实际处理可升级的 HTTP/1.1 连接。
2. 当前响应桥接统一过滤 `Connection` 和 `Transfer-Encoding`。普通 HTTP 的连接及正文分帧由 Pingora 管理，但合法的 WebSocket `101` 需要保留相应握手头，并进入持续双向转发。

Pingora 0.9.0 有升级识别与升级后的读写能力。这说明缺口在本项目适配层；也说明单独保留 `Connection` 头或单独添加 `connected(cx)` 无法解决完整连接交接。

## 传输设计

### 普通 HTTP 与升级请求分流

保留普通 HTTP 的现有桥接，在 `/ui` 请求中单独识别 Topcoat runtime WebSocket 握手。连接地址由框架运行时决定，遵循其当前文档 URL 和子协议；不新增猜测的固定 socket URL。实现时使用 v0.10.0 的公开子协议定义或已验证的协议值，不复制内部路由逻辑。

非回环客户端在进入任一分支前拒绝。Host、Origin 和握手参数先按现有保护与框架规则检查；非 Topcoat 的 upgrade、非法请求及不支持的 HTTP 版本给出明确失败，不能在失败后发送 `101`。本阶段验证 HTTP/1.1，不把 HTTP/2 extended CONNECT 记为支持能力。

### 进程内 Hyper 升级桥接

已采用有界内存双向流，让 Hyper HTTP/1.1 server 驱动一次真实握手，并调用现有 `Console::handle`。Hyper 生成 `OnUpgrade`，Topcoat 继续拥有 WebSocket 协议解析与运行时 render 管理。Pingora 对外发送已经验证的握手响应，随后在浏览器连接与内存流之间持续转发字节。

实现位于 `llmproxy-gateway/src/console/websocket.rs`：内存流为 16 KiB，两个方向的桥接 channel 容量均为 1，读块最多 16 KiB，Pingora 下行队列排空后才继续取下一块。握手等待上限为 5 秒，已升级连接不复用 HTTP keepalive。每秒检查 Pingora 停机标志，即使客户端空闲也结束桥接。`JoinSet` 随桥接结束中止仍运行的传输任务，释放内存流，框架随 socket 关闭取消 render。拒绝与错误仅记录状态和固定诊断文本。

运行时握手会在普通页面层之前被接受，因此新增路径无关的 `ConnectionGuard`，注册在 RuntimeLayer 外层。它在握手及 connected render 上检查 Host、Origin、连接开关和 `/ui` 路径范围；网关仍先检查回环客户端。框架禁止 run 改写 Host、Origin 和凭据，现有 procedure 保留 CSRF 和输入校验。

```text
浏览器 WebSocket
       │
同一 Pingora 监听端口
       │  升级响应及双向字节转发
有界进程内流
       │
Hyper HTTP/1.1 with upgrades
       │
Console::handle 与 Topcoat runtime
       │
各 shard 的独立 render
```

实施时先完成真实客户端试验，再迁移业务 shard。验收重点如下：

- Pingora 同一 Session 的读写借用及全双工调度方式；长期等待读取时，服务端主动输出仍能及时写出。
- 请求头后预读的 WebSocket 字节不丢失；桥接不先把 WebSocket 当普通 HTTP body 读完。
- `101`、`Upgrade`、`Connection`、`Sec-WebSocket-Accept` 和子协议正确传递；HTTP 拒绝响应按普通响应处理。
- 以客户端帧开始的字节流保持原样，Topcoat 负责掩码、分片及控制帧处理。
- 不创建第二个 TCP listener，不修改 Topcoat 私有类型，不维护运行时协议或浏览器脚本副本。

真实握手、双向帧、HTTP 头后的预读字节、70 KB 分片消息、Ping/Pong、Close 与 EOF 已由隔离集成测试验证。桥接使用 Pingora 公共接口，没有新增监听端口或修改框架协议。

### 生命周期与容量

桥接的缓冲区、队列和任务数须有界，并使慢消费者的背压传递到渲染输出。EOF、Close、传输错误与服务关闭均结束对应桥接、内存流和 render；已升级连接不放回 HTTP keepalive 复用。

保留 `.max_runs_per_connection(64)`。这限制同时运行的 render 数，包含 page 与 shards，不是限制文档中的 DOM 节点数。超限 render 使用框架的 `429` 错误帧，现有 render 继续工作；完成或取消后容量应释放。分页会话活跃状态订阅也计入容量，须按实际页面验证，不能直接增大上限掩盖泄漏。

## Shard 迁移顺序

| 优先级 | 范围 | 实施重点 |
| --- | --- | --- |
| 第一批 | `chat_history`、`chat_stop_button`、`chat_session_activity`、`chat_usage` | 共享连接承载实时广播；GET 首屏及时返回；每个 shard 订阅自身会话 |
| 第二批 | `chat_session_list`、协议与思考选择器 | 信号更新通过共享连接触发目标 shard；历史列表中的子订阅可独立更新 |
| 第三批 | `model_workspace`、`model_candidates`、`model_draft`、价格规则及窗口 shards | 验证父子依赖；子更新不扩大成整个模型工作区重渲染 |
| 保留 HTTP | `provider_list`、`holiday_workspace` | Provider 查询只有一个列表 shard 请求；日历翻页只有一个 procedure 和一个工作区 shard 请求，没有实时广播或多条长期连接需要合并 |

在需要连接的 shard 渲染作用域内调用 `connected(cx)`。该函数会标记内容需要连接；HTTP 首次渲染返回 false，浏览器连上后再次渲染才返回 true。`connected_untracked(cx)` 仅查询当前传输状态，不会请求连接。

迁移 `live!` 时须逐项检查当前以 HTTP method 判定首屏与长期输出的条件，确保 connected render 进入正确的订阅路径。共享 socket 不会自动消除父级对信号的依赖；需要检查父级读取、shard 参数和刷新信号，保留真正需要的父级刷新，避免子级编辑触发多次重渲染。

对话切换后，旧 render 与旧广播不能覆盖新会话的消息、生成状态或用量。停止 UI render 释放订阅，但页面导航和连接重建不取消正在进行的模型请求，也不重新提交一轮聊天。

## 重连与 HTTP 回退

Topcoat v0.10.0 运行时已有连接断开后的退避重连，连接建立后会重新启动相关 render。它不等于“WebSocket 失败后自动转成 HTTP shard”，后者需要专项验证与设计。

本项目采用以下约束：

- 短暂断线优先复用框架重连，恢复当前信号和会话快照；禁止通过自动重新发送聊天或写操作恢复界面。
- 保留 HTTP shard 更新路径，验证未请求 connection 的渲染仍可工作。
- 无升级能力或环境持续阻止 WebSocket 时，设置 `LLMPROXY_UI_WEBSOCKET=false` 后重启服务、刷新页面。开关在启动时选择，只接受 `true`、`false`，未设置默认启用；无效值导致启动失败。此模式不输出连接标记，拒绝 runtime 握手，继续使用既有 HTTP shard 和 procedure。浏览器已验证零 WebSocket、终态前文字增量、停止与用量更新。
- 回退沿用已有业务接口和校验，不自行复制 Topcoat 的 WebSocket 消息协议。框架无法支持透明切换时，记录限制，选择明确的 HTTP 模式或回滚 connected 接入。
- Host、Origin、CSRF 或非法握手被拒绝时，不通过回退绕过拒绝。

## 验收证据

验收以隔离数据库和模拟 Provider 为主；无需使用真实 Provider 凭据或向真实 OTLP 接收端发送数据。

| 场景 | 必须观察到的结果 |
| --- | --- |
| 单端口握手 | 同一 llmproxy 进程只有一个 TCP listener，运行时握手返回合法 `101` |
| 连接共享 | 聊天页多 shard 同时更新只有一条运行时 WebSocket；额外标签页独立计数 |
| 子 shard 独立更新 | 目标 shard 输出变化；父工作区和无关子 shard 的 render 次数不增加 |
| 聊天生成 | 首片段在终态前显示，停止操作可用，消息、生成状态和用量一致 |
| 路由及会话切换 | 无文档刷新；旧 render 被取消、订阅释放，无旧会话输出覆盖 |
| 断线恢复 | 当前会话恢复更新，模型请求与持久化不重复，重连不产生并存旧 socket |
| HTTP 路径 | 首屏、普通表单、procedure 及 HTTP 流式 shard 按既有约定工作 |
| 慢消费者及关闭 | 缓冲有界，双向通信无饥饿，EOF、Close 和退出后无残留桥接及订阅 |
| 容量上限 | 64 个占用 render 时额外请求被拒绝；释放一个后可继续渲染 |
| 访问保护 | 非回环、错误 Host、跨站 Origin、错误 CSRF 被拒绝，握手和 run 不能扩大可访问路由 |
| 代理兼容 | 同端口 `/v1/*`、SSE 和订阅代理入口保持既有行为 |

记录测试命令、浏览器 WebSocket 数、文档 loader 身份、render 计数、取消和资源释放证据。只看到 `101` 或只看到页面变化不足以判定完成，不预设性能提升百分比。

## 技术依据

- [Topcoat v0.10.0 发布说明](https://github.com/tokio-rs/topcoat/releases/tag/v0.10.0)：共享连接、子 shard 独立渲染与 64 个同时 render 限制。
- 锁定依赖 Topcoat 0.10.0 的 `runtime/src/connection.rs`、`runtime/src/layer/socket.rs`、`runtime/browser/src/render/connection.ts`：connection 标记、握手、run 调度与断线重连。
- 锁定依赖 Topcoat Router 0.10.0 的 `content/websocket/upgrade.rs`：Hyper `OnUpgrade` 与握手要求。
- 锁定依赖 Pingora Core 0.9.0 的 `protocols/http/server.rs`、`protocols/http/v1/server.rs`：升级识别、响应与读写状态。
- 项目 `llmproxy-gateway/src/console.rs`、`llmproxy-console/src/lib.rs`、`llmproxy-console/src/app.rs` 和 `llmproxy-console/src/app/ui/chat.rs`：当前桥接、保护与 HTTP shard 路径。
