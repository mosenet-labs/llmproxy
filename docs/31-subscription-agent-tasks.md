# 独立订阅代理实施任务

需求、边界与设计见[文档 30](30-subscription-agent-requirements-and-design.md)。日期：2026-10-07。

状态：`[x]` 已完成，`[ ]` 未完成。诊断、模拟验证和真实验收分别记录；不以接口存在代替运行验收。

## 实施顺序与验收

| 任务 | 依赖 | 交付及验收 |
| --- | --- | --- |
| A1 设计及任务落地 | 无 | 固定后端策略、适配边界、状态机、禁止降级及逐项验收入文档 |
| A2 独立二进制及 Codex 能力诊断 | A1 | 独立构建；检测稳定／实验 schema；报告 ephemeral、动态工具、原始历史注入能力；缺失和未验证条件明确，不读取登录凭据、不发起推理 |
| A3 Responses 公共协议边界 | A1 | 复用 Core 请求、响应、SSE 类型；无状态参数校验；上游流式请求准备；真实终态聚合非流式；媒体、工具、推理、usage 和未知扩展保留；截断／失败不伪装成功 |
| A4 OAuth 登录与直连后端 | A3 | 官方动态注册、loopback callback、state／nonce／PKCE、JWKS 验证、授权范围、受保护凭据、串行刷新；真实账号授权及请求；不使用 Codex token 替代 OAuth |
| A5 Codex 运行约束验证 | A2 | 原生认证加载与刷新；检查正常／失败／取消／崩溃的正文落盘；全部内置工具在执行前禁止；不创建 thread/turn，工具交付后释放请求，下一轮完整历史及多工具 ID 保留；不足时记录需要的 Codex 扩展，不擅自降低需求 |
| A6 Codex Responses 后端 | A3、A5 | stdio RPC 关联、无会话原生 HTTP 传输、完整上下文及媒体保留、客户端工具、reasoning／usage／终态和取消；错误不切换后端 |
| A7 共用请求服务及本地 HTTP | A4；Codex 启用依赖 A6 | 启动时唯一后端；鉴权、容量、超时及有界输出；流式／非流式实际 HTTP；多对话隔离，客户端断开取消，不保存历史 |
| A8 反向通道及远程转接 | A7 | 采用同端口 HTTP 长轮询及 NDJSON 上传；节点出站连接、请求／响应流／取消／租约／重连；本地和远程共用容量；断线失败，无推理重放；受保护 HTTP 入口接入普通 Provider |
| A9 注册、健康及服务准入 | A8 | 稳定身份、双数据库登记、独立健康与准入状态、控制台展示和启停；默认不加入，启用关联普通 Provider，重连幂等，停用和模型更新有明确行为 |
| A10 完整验收及运行说明 | A4–A9 | 两个模式分别验收；远程 NAT 场景、媒体／工具双轮、并发、取消、慢客户端、重启、凭据及正文脱敏；完整 workspace 验证及打包说明 |

## 当前进度

- [x] A1：更新已确认设计，并拆分依赖和验收任务。
- [x] A2：独立二进制和能力诊断；本机接口检测通过，运行约束仍明确为未验证。
- [x] A3：Responses 公共协议边界；模拟协议测试通过，尚未连接真实上游。
- [x] A4：真实 OAuth 动态注册、签名／范围校验、模型目录、实际本地 HTTP 流式／非流式请求及重启后的刷新轮转均已验收。
- [x] A5：受控进程工具阻断、全过程正文隔离、原生认证加载、真实 AuthManager 刷新轮转和重启复用已验收。
- [x] A6：受控无会话 RPC、有界并发／取消、原生 HTTP 状态／SSE、完整历史及工具 ID；独立二进制真实 HTTP 链路通过。
- [x] A7：两个后端共用服务和本地 HTTP；OAuth 模拟上游及 Codex 受控真实进程 HTTP 链路通过。
- [x] A8：反向长轮询、NDJSON 流、租约、取消及重连；本地回环转接测试，跨地域 NAT 留在 A10。
- [x] A9：双数据库迁移、身份登记、健康展示、准入与普通 Provider 关联；SQLite、实际 PostgreSQL 和控制台 HTTP 验证。
- [ ] A10：完整验收及运行说明。

验证规则：开发中运行受影响测试，交付前统一运行一次完整验证。不自动提交或推送，不修改其他进行中的聊天历史工作。

## 首批实现记录

新增 workspace 成员 `crates/llmproxy-subscription-agent`，可独立构建，未改变默认 `llmproxy` 构建入口，也未引入新的第三方 crate 版本。

```sh
cargo build -p llmproxy-subscription-agent
cargo run -p llmproxy-subscription-agent -- doctor
cargo run -p llmproxy-subscription-agent -- doctor --codex-bin /path/to/codex
```

`doctor` 只调用 `generate-json-schema --experimental`，有超时、子进程回收和临时 schema 清理。输出三个接口检查及四个独立的运行验收条件；没有登录、创建 thread 或发起模型推理。退出码 `0` 为条件通过，`1` 为诊断失败，`2` 为条件缺失或未验证。当前正常诊断返回 `2`，不能据此启用 Codex 服务。

本机诊断结果：`ephemeral_thread`、`dynamic_tools`、`raw_history_injection` 可用；`native_authentication`、`no_conversation_persistence`、`no_builtin_tool_execution`、`stateless_tool_roundtrip` 未验证，`backend_ready:false`。

Responses 边界使用 Core 原有结构与 SSE 分帧，不增加另一套协议类型。OAuth 请求准备保留原数组历史、媒体、函数调用／结果、推理及扩展字段，将纯文本转换为等价 user 输入，固定上游 `store:false`、`stream:true`，分别记录对外响应模式；明确拒绝无状态不支持及当前 SIWC 不支持的参数。

非流式收集器只采用真实终态的完整 Response，保留 `failed`／`incomplete`，不累计文本后拼造成功。检查帧容量、截断、流内错误、响应 ID 串线、终态状态不匹配和终态后多余数据；失败后不可复用。

首批 9 项测试通过，涵盖能力与就绪区分、旧版／坏 schema、子进程失败脱敏与超时、媒体工具历史保留、参数拒绝、分块 SSE、真实 usage、错误与未完成状态和容量边界。本机诊断执行通过，未进行真实账号推理。

完整验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings` 和差异检查通过。`cargo test --workspace --offline` 首次因沙箱禁止回环监听中断；在允许本地监听的环境下重跑通过，354 项通过、11 项既有环境／人工验收用例忽略。未修改其他聊天历史相关改动。

首批记录是此前阶段结果；以下为本轮实现及当前实际边界。


## 本轮实现及运行说明

已实现 OAuth `login` 和直连服务、本地 HTTP、节点出站反向连接、网关独立登记／租约／转接、SQLite/PostgreSQL 迁移、准入操作和控制台页面。代码并未绕过 A5 来启用 Codex。

构建独立可执行文件：

```sh
cargo build --release -p llmproxy-subscription-agent
# 输出 target/release/llmproxy-subscription-agent
```

先准备 0700 的状态目录和 0600 的本地访问密钥文件，密钥至少 32 字符。登录会自动打开系统浏览器并监听回环回调；同一目录只能被一个 login/serve 进程持有。

```sh
llmproxy-subscription-agent login --state-dir /YOUR/PRIVATE/agent-state
llmproxy-subscription-agent serve --backend chatgpt-oauth \
  --state-dir /YOUR/PRIVATE/agent-state --listen 127.0.0.1:8787 \
  --key-file /YOUR/PRIVATE/local-key --models YOUR_AUTHORIZED_MODEL --concurrency 4
```

HTTP 接口为 `POST /v1/responses`、`GET /v1/models` 和 `GET /health`，都使用 `Authorization: Bearer <本地密钥>`。模型目录是显式声明的允许列表，不是已验证的账号模型授权；实际拒绝返回上游错误，健康仅在真实请求后更新，不定期发起推理探测。客户端提交完整 `input`，使用 `store:false`；`stream:false` 在节点聚合真实终态。默认单次执行超时 300 秒，请求读取 30 秒，正文和单帧 16 MiB，输出队列 8 项。

llmproxy 远程网关配置以下环境变量，两者至少 32 字符且不同：

```text
LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY=<节点首次接入的注册密钥>
LLMPROXY_SUBSCRIPTION_RELAY_KEY=<网关内部固定转接入口密钥>
```

节点只需要注册密钥，不需要内部转接密钥，也不向网关发送 OAuth 凭据。公网使用 HTTPS，可在现有网关前配置 TLS 入口。HTTP 仅允许节点连接本机 loopback。节点启动追加：

```sh
  --remote https://YOUR.LLMPROXY.HOST \
  --registration-key-file /YOUR/PRIVATE/registration-key --name personal-device
```

访问 llmproxy `/ui/subscriptions`，首次登记默认停用；点击允许加入后生成普通 Responses Provider，再在 Models/Model Routes 配置模型与路由。停用解除现有绑定，后续重新启用需要重新确认路由。心跳 10 秒、租约 60 秒；取消通过心跳列表传递，断线结束所有当前远程请求，重连退避 2–30 秒。本地请求不因远程断线被取消，成功结束的请求不发送取消消息。

凭据文件包含 OAuth token、账号标识和稳定 host；节点身份另存 `node.json`，两者是允许落盘的身份数据。不要将状态目录或密钥文件放入版本控制。对话不写入这些文件，不保留服务端历史。当前 owner-only 权限校验在 Unix 上实现；Windows 的 ACL 验收尚未完成。

本轮测试覆盖 RSA 签名正例／篡改、audience／nonce／时间／未知公钥、私有文件及独占锁、托管工具拒绝、真实终态／截断、模拟 HTTP 流式／非流式／取消／容量恢复、节点登记和重连、Provider 准入关联、旧租约失效、控制台密钥隐藏和 CSRF。测试密钥与 JWT 是合成公共验证素材，未保存真实账号凭据。

仍需完成：A4 真实登录和旋转刷新；A5 真实 Codex 账号刷新轮转；A10 跨地域部署、真实图片／文件／工具双轮、长时慢客户端／网络中断验收。没有可用的该程序 OAuth 授权及远程部署环境时，不把模拟结果登记为真实账号／NAT 通过。


验证记录：本轮完整 `cargo test --workspace --offline` 通过，既有人工／真实环境用例保持忽略；随后补充的节点响应前断开取消、本地 HTTP 鉴权和容量测试单独通过。并行构建期间一次新集成测试的 5 秒启动等待超时，单独复跑通过，已将仅测试使用的启动等待调整为 20 秒。后续本轮已通过 PostgreSQL 节点专项测试，详见补充验证记录。


最后检查：独立二进制构建及 `--help` 通过；`serve --backend codex` 按预期明确失败，未读凭据、未启动模型、未自动切换。新增独立代理 14 项测试通过，包含真实出站 worker 协议；Store 节点专项测试和 Gateway 注册／转接／响应前断开取消／控制台专项通过。`cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 通过。后续通过 Docker 中 PostgreSQL 18.3 完成节点专项，详见补充验证记录。未自动提交、合并或推送。

## 补充确认及验证记录（2026-10-07）

- 用户明确允许受控 Codex 扩展／补丁；已落地固定官方源码哈希、最小工具策略补丁、构建脚本和说明。源码哈希校验及补丁应用检查通过，篡改压缩包拒绝测试通过。源码 cargo check 已通过；策略执行测试及真实运行约束检查仍需完成，A5 不勾选。补丁还加入进程启动隔离，忽略用户／项目配置并关闭 SQLite 日志、遥测及其重载器；策略标志和启动模式必须一致。
- A6 stdio RPC 两项新增测试通过：原版／部分策略确认不允许推理，事件先于 RPC 响应时保留且响应 ID 错误不会回显上游秘密。独立代理共 16 项测试通过。
- PostgreSQL 节点专项实际运行，修复 concurrency 的 INTEGER/BIGINT 类型不匹配；SQLite 与 PostgreSQL 注册／重连／准入／身份专项均通过。隔离 schema 及临时 PostgreSQL 容器已删除。PostgreSQL 用例默认忽略，显式提供 LLMPROXY_TEST_DATABASE_URL 后执行。
- 用户授权真实 OAuth 验收，已实际打开登录流程；callback 等待超时，尚未取得授权。已将后续等待延长至 900 秒，正在确认浏览器页面状态。未使用 Codex 登录缓存替代授权，不登记为真实账号验收通过。
- 慢客户端背压后断开释放容量测试通过；工作区 clippy（全部 target，-D warnings）、fmt 和 diff 检查通过。完整工作区 cargo test --workspace --offline 通过；人工／真实环境用例维持默认忽略，PostgreSQL 节点专项已另外显式执行通过。

## A5/A6 后续实施步骤：受控无会话 RPC

1. Codex 扩展专用 capabilities／Responses／cancel RPC，复用原生认证及区域路由，HTTP status 和 SSE 字节传递；验证：源码检查及真实本机 mock HTTP 的状态／字节／多工具往返，不调用 thread/turn。
2. 独立代理增加有界 RPC 请求关联和唯一 codex 后端，复用既有终态校验、本地 HTTP 和反向注册；验证：并发隔离、错误不切换、断开取消及容量释放，原版拒绝。
3. 实际受控进程在正常／错误／取消／崩溃后扫描隔离目录，验证正文不落盘；原生认证只报告有效状态，不回显凭据。验证通过后启用，并更新 A5/A6 状态。

旧 thread/start ToolPolicy 试验已完成编译和两个策略执行测试；最终路径改为不创建会话的 RPC，避免补齐有损 turn 通知和额外执行路径。


## 当前可运行状态与最终验证

默认 codex 后端已实现。必须使用本项目受控构建，通过 --codex-bin 指定；程序先导出并检查受控策略描述，再启动 app-server 确认版本、无会话、客户端工具和原生登录状态。原版在服务器启动前拒绝。doctor 执行同样的无推理检查；运行隔离审计由 verify_runtime.py 独立验证，不用能力确认字段代替审计。上文首批诊断及拒绝启动记录属于历史阶段。

本机实际受控 Codex + release 独立代理 + 本机 HTTPS 上游链路通过：流式／非流式、原始 HTTP 错误与扩展字段、完整双轮工具历史、多工具 ID、usage、并发隔离、慢消费者、有界帧、取消、EOF 和强制退出；隔离目录正文扫描及命令／MCP 执行标记检查通过。该验收使用合成原生登录数据。

运行命令：

    python3 crates/llmproxy-subscription-agent/codex/verify_runtime.py --codex-bin /PATH/TO/CONTROLLED/codex --agent-bin /PATH/TO/llmproxy-subscription-agent
    llmproxy-subscription-agent serve --backend codex --codex-bin /PATH/TO/CONTROLLED/codex --state-dir /YOUR/PRIVATE/agent-state --listen 127.0.0.1:8787 --key-file /YOUR/PRIVATE/local-key --models YOUR_AUTHORIZED_MODEL

OAuth 用 --backend chatgpt-oauth，先执行 login；不自动切换。节点后端身份固定，切换后端需使用新的 state-dir，以新节点注册，避免覆盖原登记身份。远程准入默认关闭，由控制台独立启用，并复用普通 Provider 与模型／路由配置。

A4/A5/A10 未勾选原因：真实 OAuth 已启动两次但 callback 超时，未取得授权；真实 Codex 原生账号刷新轮转、实际模型／媒体请求及跨地域 NAT 环境尚未验收。需要浏览器授权状态和远程 HTTPS 地址／本机注册密钥文件路径；不应粘贴凭据。已有代码与本机模拟结果不能代替这些外部验收。


最终验证：cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all --check、git diff --check 全部通过。独立代理 19 项测试、受控 Codex 2 项专项测试、固定源码补丁干净应用及压缩包篡改拒绝测试通过；更新后的实际 Codex／agent HTTP 链路复验通过。PostgreSQL 专项此前已在隔离容器显式执行通过，默认测试不自动连接外部数据库。

当前本机产物：

- target/release/llmproxy-subscription-agent：独立 release 二进制。
- target/controlled-codex/codex：经过验收的受控 Codex debug 构建；未替换已安装 Codex。正式 release 构建由 codex/build.py 提供，尚未在本机执行该 release 构建。

本机复制后的受控 Codex doctor 返回 backend_ready:true、native_authentication:available。只确认原生登录可用，不发起模型请求；三项隔离／工具审计仍标为 unverified，运行审计结果单独记录，真实刷新轮转仍待验收。


## 2026-10-08 A4 重试与 host ID 修复

真实登录返回 invalid_authorize_request，param 为 ext_agent_host_id。旧实现错误地使用了 64 字符随机十六进制标识。官方要求 UUID 方案为 urn:uuid:<带连字符的 UUIDv4>；已改为生成并持久化此 URI，保持后续登录稳定。没有 OAuth 注册的旧十六进制标识自动迁移；已有注册身份禁止自动改动。

依据：[官方 Host-ID 格式](https://developers.openai.com/siwc/token-sharing-open-source)。该错误发生在授权页面校验阶段，没有取得授权凭据；修正后重新启动登录，真实授权／推理验收仍待完成。


## 2026-10-08 A4 真实验收完成

修正 host ID 后真实浏览器注册成功，授权回调、issued client ID、ID token 签名／issuer／audience／nonce 和 granted scopes 校验通过。实际 GET https://api.openai.com/v1/models 返回 HTTP 200，账号目录包含 gpt-6.1-sol 等模型。选择目录中的 gpt-6.1-sol 做最小推理，不调用 Codex 凭据。

真实 SIWC Responses 返回有效 SSE，但实际 HTTP 200 未携带 Content-Type，导致最初的本地代理拒绝。请求现在显式设置 Accept:text/event-stream；若响应缺少 Content-Type，仍进入有界 SSE 解码和真实终态校验，不能将无效正文或缺失终态视为成功；明确非 SSE 类型继续拒绝。补充实际 HTTP 回归覆盖缺失类型有效 SSE、缺失类型无效正文、显式错误类型。

通过 release 独立二进制、本机 HTTP、真实公共 Responses 上游验证：

- stream:false：HTTP 200、status completed，真实 usage 为 input_tokens 11、output_tokens 5、total_tokens 16。
- stream:true：收到唯一真实 response.completed，没有 failed／incomplete／error，同样真实 usage。
- 停止进程，将本地到期元数据设为过期后重启：成功触发真正 OAuth 刷新，再次推理 completed；refresh token 发生轮转，到期时间恢复有效，账号 subject、issued client ID 与 host ID 不变。
- oauth.json 权限 0600，刷新事务临时文件已清理；临时验收 HTTP 密钥及服务进程已清理。凭据只保存在被忽略的受保护状态目录，未写入文档或工具输出。
- 独立代理 20 项测试、包完整测试、Clippy、fmt 和 diff 检查通过，release 二进制已重新构建。

A4 已勾选完成。前文 OAuth 超时和未验收记录属于此前阶段。A5 的真实 Codex 原生账号刷新与 A10 的跨地域 NAT／媒体工具综合验收仍未完成；本次 OAuth 刷新不能代替 Codex AuthManager 的刷新验收。


## 2026-10-08 A5 真实验收完成

使用独立 CODEX_HOME=data/codex-a5 的原生 Codex 浏览器登录创建验收会话；没有复制或改动当前桌面会话凭据，也没有使用 A4 OAuth token 替代。原生认证文件为 data/codex-a5/auth.json，权限 0600。代理使用独立 state-dir=data/subscription-agent-codex-a5。临时 HTTP 密钥和验收服务均已清理，认证文件保留供后续使用。

首先发现原生 Codex 不接受 gpt-6.1-sol：返回 HTTP 400，detail 明确说明该模型不支持 ChatGPT 账号的 Codex 使用。实际原生模型目录 GET /backend-api/codex/models 返回 HTTP 200，选择其中 gpt-6-astra 完成验收。两个后端模型目录不可混用；不自动切换后端，也不隐藏上游模型拒绝。

发现并修复受控 RPC 在 Content-Type 缺失时补造 application/octet-stream 的问题。DTO 改为 nullable contentType，原生桥接保留缺失值；代理使用已有的有界 SSE／真实终态验证，明确错误类型仍失败。新增 RPC 回归验证缺失头不被补造。补丁已同步，固定源码哈希及干净目录应用检查通过。

实际 release 代理 + 受控 Codex + 原生上游验收：

- 正常非流式：HTTP 200、completed，真实 usage 为 input_tokens 11、output_tokens 5、total_tokens 16。
- 停止服务，仅失效独立会话本地 access token，并将 last_refresh 设为过去，保留该会话的真实 refresh token。Codex AuthManager 在请求前执行原生刷新；没有自行调用刷新接口或替换为另一个后端。
- 刷新后的流式请求：HTTP 200，唯一真实 response.completed；access token 和 refresh token 均变化，last_refresh 更新，原生 account_id 不变。
- 再次重启代理和受控 Codex，使用落盘后的新认证完成非流式请求，HTTP 200／completed。
- 认证权限保持 0600；扫描 CODEX_HOME 和代理状态目录未发现验收提示词、sessions 或 history.jsonl 文件。认证保存属于允许的凭据持久化，不是对话持久化。
- 更新后的实际进程隔离审计再次通过：命令／文件／thread RPC 执行前拒绝、配置 MCP 未启动、完整双轮历史和多工具 ID、取消、stdin EOF、强制退出及正文扫描均通过。
- 独立代理完整包测试 20 项、Clippy、fmt、diff 检查通过，release 代理与受控 debug Codex 已重建。

A5 已勾选。原生模型可用性及头部缺失均为实际发现；原版 Codex 仍在服务器启动前拒绝。当前受控产物为 target/controlled-codex/codex，未替换已安装 Codex。A10 的跨地域 NAT、实际媒体／工具综合与长时网络验收仍待完成。
