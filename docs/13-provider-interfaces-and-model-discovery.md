# 多协议 Provider 与模型探测

关联：[原始需求](00-original-requirements.md) · [Provider 控制台](07-provider-console.md) · [任务](02-tasks.md#w多协议-provider-与模型探测)。

## 已确认的行为

一个 Provider 可以同时支持 OpenAI Chat、OpenAI Responses 和 Anthropic Messages；至少选择一种。每个协议独立配置上游路径，默认依次为 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`。这些是上游路径；客户端入口仍固定为原有三个 `/v1/...` 路由。网关按已匹配的协议读取该 Provider 的路径，保留客户端查询参数，并在 Pingora 的 `upstream_request_filter` 中改写请求 URI。认证头仍按协议生成。

“设为当前服务”按协议执行：同一 Provider 可以被多个协议绑定，不同协议也可以绑定不同 Provider。启用只表示可选择；停用会解除此 Provider 的所有绑定。编辑当前 Provider 的路径会在下一轮快照刷新后影响新请求；正在进行的 SSE 保持原配置。移除正在绑定的协议必须先切换或停用。

当前请求按协议绑定选择 Provider；模型 ID 与 Provider 的映射留待后续实现。

模型探测有独立路径，默认 `/models`，可修改，例如 `/v1/models`。探测协议只能选择该 Provider 已勾选的接口协议；选择 OpenAI Chat 或 Responses 时使用 Bearer Token，选择 Anthropic Messages 时使用 `x-api-key` 和 `anthropic-version`。编辑表单的三个协议开关始终以真实的保存状态显示，并允许切换；至少须保留一个协议。

表单中的“探测”使用当前填写的地址、模型路径、协议和凭据预览请求，尚未保存的配置也能探测。编辑时 API Key 留空会在服务端使用已保存的加密凭据；浏览器不读取旧 Key，也不直接访问上游。探测成功显示绿色勾，失败显示红色叉，未探测显示灰色；模型 ID 或错误信息包含 Provider 名称。预览结果只保留在当前表单，保存表单后才将状态写入数据库，重新打开时回显。修改地址、凭据、协议或探测路径会清除旧状态。列表不提供探测操作。探测状态不影响路由绑定。

## 数据与迁移

Toasty 的 `providers` 行包含三个可空协议路径、`models_path`、`models_protocol` 和 `models_probe_status`；空路径字段表示不支持该协议。`route_bindings` 仍按协议维护单一当前 Provider。SQLite 与 PostgreSQL 分别有追加迁移；旧记录依据原 `protocol` 回填对应的默认路径，并按旧 `models_auth` 与已配置协议回填探测协议。原 `protocol` 与 `models_auth` 字段随后移除；旧绑定不变。没有跨数据库迁移。

路径校验要求单个 `/` 开头的站内绝对路径，不含域名、查询参数、片段、反斜杠、空白、控制字符或 `.`/`..` 段；长度最多 2048 字节。上游根地址仍由 `host / port / tls` 保存。模型探测不跟随重定向，最多读取 1 MiB 响应，最多展示 100 个模型 ID。失败只向用户展示状态或通用连接错误，不回显凭据与上游响应体。

## 验收

1. 旧 SQLite 与 PostgreSQL Provider 升级后仍有原协议默认路径、原凭据和原绑定。
2. 一个 Provider 可绑定多个协议；不同下游入口发往各自配置的上游路径，查询参数、认证头和 SSE 行为保持正确。
3. 表单可编辑三个协议路径及模型列表路径；未配置协议不可设为当前，移除已绑定协议得到冲突。
4. 表单预览探测使用当前填写的配置；预览期间数据库不变，保存后结果状态可在重新打开时看到。
5. 选择不同探测协议后自动使用对应的鉴权头；成功显示模型 ID 与绿色勾，失败显示 Provider 名称、原因与红色叉，日志不包含凭据。
