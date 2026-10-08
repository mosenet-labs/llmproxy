# llmproxy-subscription-agent

独立运行的订阅代理，将 ChatGPT 订阅能力提供为 Responses HTTP API，也可主动连接远程 llmproxy，注册为可配置的 Provider。

每个进程只使用一个后端，不自动切换：

| 后端 | 认证来源 | 运行依赖 |
| --- | --- | --- |
| `codex`（默认） | Codex 原生登录，由 Codex 加载和刷新 | 代理二进制、受控 Codex 二进制 |
| `chatgpt-oauth` | 本程序的 Sign in with ChatGPT 授权 | 代理二进制 |

请求携带完整历史，代理不保存对话正文，也不等待下一次请求的工具结果。工具由客户端执行，上游不支持的参数明确报错。支持流式响应及依据真实终态聚合的非流式响应。

## 构建

以下命令均从 llmproxy 仓库根目录执行：

```sh
cargo build --release -p llmproxy-subscription-agent
AGENT_BIN="$PWD/target/release/llmproxy-subscription-agent"
```

`codex` 后端必须使用本项目的受控 Codex，原版会在 app-server 启动前被拒绝。构建方式见 [codex/README.md](codex/README.md)：

```sh
python3 crates/llmproxy-subscription-agent/codex/build.py \
  --work-dir /tmp/llmproxy-controlled-codex
```

构建脚本需要 Python 3.12+ 和 Rust 1.98.1，完成后输出受控 Codex 的可执行文件路径。将该路径设置为 `CODEX_BIN`，例如：

```sh
CODEX_BIN="/tmp/llmproxy-controlled-codex/codex-1199b3976277424fbc5035b55ca11ce69c860a69/codex-rs/target/release/codex"
```

运行时无需携带 `codex/` 源码、补丁、构建脚本，也不依赖 Python。受控 Codex 不替换机器已安装的 Codex。

## 准备本地配置

以下示例适用于 macOS/Linux。状态目录权限为 `0700`，密钥文件权限为 `0600`。

```sh
AGENT_CONFIG_DIR="$HOME/.config/llmproxy-agent"
umask 077
mkdir -p "$AGENT_CONFIG_DIR/codex-state" "$AGENT_CONFIG_DIR/oauth-state"
chmod 700 "$AGENT_CONFIG_DIR" "$AGENT_CONFIG_DIR/codex-state" "$AGENT_CONFIG_DIR/oauth-state"

# 首次生成本地 HTTP 访问密钥，已有文件时保留原值。
if [ ! -e "$AGENT_CONFIG_DIR/local-key" ]; then
  openssl rand -hex 32 > "$AGENT_CONFIG_DIR/local-key"
fi
chmod 600 "$AGENT_CONFIG_DIR/local-key"
```

## 使用已有 Codex 登录启动

`CODEX_HOME` 指向已有 Codex 登录目录，通常为 `$HOME/.codex`。受控 Codex 使用原生认证存储机制，兼容 keyring 和 `auth.json`，由它负责刷新；认证凭据不会上传给远程 llmproxy。

```sh
CODEX_HOME="$HOME/.codex" "$AGENT_BIN" doctor --codex-bin "$CODEX_BIN"

CODEX_HOME="$HOME/.codex" "$AGENT_BIN" serve \
  --backend codex \
  --codex-bin "$CODEX_BIN" \
  --state-dir "$AGENT_CONFIG_DIR/codex-state" \
  --listen 127.0.0.1:8787 \
  --key-file "$AGENT_CONFIG_DIR/local-key" \
  --models gpt-6-astra \
  --concurrency 4
```

`doctor` 不发起推理，仅检查受控扩展和原生认证状态；不能代替运行隔离审计。退出码 `0` 表示后端可用，`1` 表示诊断失败，`2` 表示后端条件不满足。

## 使用本程序 OAuth 启动

先执行登录，在浏览器完成 ChatGPT 授权，再启动服务：

```sh
"$AGENT_BIN" login --state-dir "$AGENT_CONFIG_DIR/oauth-state"

"$AGENT_BIN" serve \
  --backend chatgpt-oauth \
  --state-dir "$AGENT_CONFIG_DIR/oauth-state" \
  --listen 127.0.0.1:8787 \
  --key-file "$AGENT_CONFIG_DIR/local-key" \
  --models gpt-6.1-sol \
  --concurrency 4
```

授权文件保存在对应状态目录的 `oauth.json`，程序自动刷新凭据。此模式不使用 Codex 的登录文件，也不接受 `--codex-bin`。

两种后端的模型目录可能不同。示例模型需以账号实际可用模型为准；`--models` 是允许列表，不会自动探测模型权限，可用逗号分隔多个模型。上游模型拒绝会原样返回。

## 注册到远程 llmproxy

远程 llmproxy 需同时设置两个不同、各至少 32 字符的密钥并重启；样本见仓库根目录的 [.env.example](../../.env.example)：

```sh
LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY=<注册密钥>
LLMPROXY_SUBSCRIPTION_RELAY_KEY=<内部转接密钥>
```

可分别执行两次 `openssl rand -hex 32` 生成。注册密钥由远程服务与节点共用；转接密钥仅用于 llmproxy 内部转接，无需交给节点。

将远程的注册密钥保存到节点文件中，替换下面的占位值。若远程已配置，不要在节点另行生成不同的注册密钥：

```sh
umask 077
printf '%s\n' '<与远程配置一致的注册密钥>' > "$AGENT_CONFIG_DIR/registration-key"
chmod 600 "$AGENT_CONFIG_DIR/registration-key"
```

以 Codex 后端为例，启动时增加远程配置：

```sh
CODEX_HOME="$HOME/.codex" "$AGENT_BIN" serve \
  --backend codex \
  --codex-bin "$CODEX_BIN" \
  --state-dir "$AGENT_CONFIG_DIR/codex-state" \
  --listen 127.0.0.1:8787 \
  --key-file "$AGENT_CONFIG_DIR/local-key" \
  --models gpt-6-astra \
  --remote https://YOUR-LLMPROXY \
  --registration-key-file "$AGENT_CONFIG_DIR/registration-key" \
  --name personal-device
```

OAuth 模式同样支持这三个远程选项。访问远程 `/ui/subscriptions`，确认节点在线后启用“允许加入代理服务”，再为生成的普通 Responses Provider 配置模型与路由。首次注册默认不参与代理，重新连接保留准入设置。

节点主动发起连接，本机无需开放入站端口。节点身份保存在状态目录，重启应复用该目录；切换后端使用不同的状态目录。远程断线不会重放正在执行的推理。

## 本地 HTTP 调用

本地服务提供 HTTP，不直接提供 HTTPS。以下三个接口均需 Bearer 鉴权：

| 接口 | 用途 |
| --- | --- |
| `GET /health` | 后端健康与并发配置 |
| `GET /v1/models` | 启动时配置的模型允许列表 |
| `POST /v1/responses` | 流式或非流式 Responses 请求 |

例如，在使用 Codex 示例模型启动后：

```sh
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H "Authorization: Bearer $(cat "$AGENT_CONFIG_DIR/local-key")" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gpt-6-astra","input":"Reply with exactly OK.","store":false,"stream":true}'
```

改为 `stream:false` 可获取聚合后的 JSON。客户端每次提交完整上下文，工具执行后在下一次请求提交完整历史与工具结果；不支持 `previous_response_id`、`conversation` 等服务端会话续接。

本地访问密钥与远程注册密钥用途不同，当前即使启用远程注册，也必须提供本地访问密钥。需要跨设备直接访问本地入口时，应通过 HTTPS 反向代理或加密隧道；`--remote https://…` 仅表示出站连接使用 HTTPS。

## 验证与验收状态

```sh
cargo test -p llmproxy-subscription-agent
python3 crates/llmproxy-subscription-agent/codex/verify_runtime.py \
  --codex-bin "$CODEX_BIN" --agent-bin "$AGENT_BIN"
```

真实 OAuth 和原生 Codex 的登录、推理、刷新轮转及重启复用已验收；受控进程工具阻断、取消／异常退出和正文隔离已验收。跨地域 NAT、真实媒体／工具综合及长时网络验收仍属于待完成的 A10，详见[任务清单](../../docs/31-subscription-agent-tasks.md)和[需求与设计](../../docs/30-subscription-agent-requirements-and-design.md)。


## 注册排查

启动日志和 /health 的 backend 表示实际后端；指定 codex 不会自动切换为 chatgpt-oauth。注册成功会输出“节点已注册”，失败会输出连接／超时／HTTP 状态并重试，不输出密钥。

如果控制台显示“远程节点接入尚未配置”，请在 llmproxy 服务端的 .env（不是 .env.example）或进程环境中设置两项订阅密钥，并重启 llmproxy。只在 agent 所在终端设置变量不会改变已经运行的 llmproxy。注册密钥文件内容必须与服务端注册密钥一致；首次注册后仍需手动允许加入代理服务。

### 节点名称与模型导入

`--name` 为可选参数；首次注册未提供名称时，服务端使用 `node-` 加节点随机身份的 6 位短 hash，例如 `node-a83f2c`。名称可以在订阅节点页面编辑，并同步更新关联 Provider；agent 重连不会覆盖服务端名称。

`--models gpt-6-astra,gpt-6-sol,gpt-6-luna` 可以声明多个模型。允许节点加入服务后，页面查询该 Provider 的模型列表，勾选需要导入的模型。已关联的节点也可点击“导入模型”补充导入；已导入模型不会重复创建。模型导入后仍需配置 Model Routes。
