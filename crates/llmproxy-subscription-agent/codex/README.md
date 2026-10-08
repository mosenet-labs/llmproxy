# 受控 Codex 依赖

用户已授权受控扩展／补丁。本目录固定官方修订、压缩包和原始文件 SHA-256，不修改已安装的 Codex。

```sh
python3 crates/llmproxy-subscription-agent/codex/build.py --work-dir /tmp/llmproxy-controlled-codex
```

构建需要 Python 3.12+、Rust 1.98.1 和公开依赖。脚本校验源码、应用 stateless-responses.patch、执行专项测试并构建独立 Codex。已有源码目录不会被重置。--archive PATH --prepare-only 可验证固定压缩包及补丁应用。

代理设置 LLMPROXY_CODEX_PROXY_V1=1，补丁冻结启动模式：忽略用户／项目配置，关闭 SQLite 状态及日志、反馈记录、遥测及重载、模型刷新和插件启动任务；保留原生认证存储、刷新、账号区域路由和受管网络策略。认证使用原生 auto 存储，读取 keyring，缺失或不可用时读取本地 auth.json。对话只在内存及 HTTP/stdio 传输中存在。

专用 RPC：

- llmproxy/capabilities：确认 policyVersion:1、stateless:true、clientToolsOnly:true 和 nativeAuthReady；不发起推理。
- llmproxy/responses：接受完整 Responses request，固定 store:false、stream:true；返回真实 HTTP status/contentType；上游缺少类型时 contentType 为 null，代理不补造类型，仍校验 SSE 与真实终态。llmproxy/chunk 通知按 requestId 关联，data 为最多 32 KiB 的原生字节的 base64；end 标识真正 EOF，error 标识异常。
- llmproxy/cancel：取消同连接 requestId；连接关闭也取消相关请求。

该模式只允许上述请求及初始化。thread/turn、命令、文件、MCP 和其他 RPC 均在分派前拒绝。工具声明仅允许 function/custom/namespace，托管工具在 HTTP 前拒绝；函数名与内置工具同名也只是模型输入数据。没有 thread、turn、工具执行器或工具结果等待。HTTP 只对明确的 401 调用原生认证恢复，不重放网络中断的模型请求。

独立代理先通过纯 schema 导出的 llmproxy-proxy-policy.json 检查受控构建，再启动服务器确认这一组能力；原版 Codex、未登录和部分确认明确失败，不切换 OAuth。客户端提交全量历史，工具 ID 和结果原样传递。上游不支持的参数由上游明确拒绝。

```sh
python3 crates/llmproxy-subscription-agent/codex/verify_runtime.py --codex-bin /PATH/TO/CONTROLLED/codex --agent-bin /PATH/TO/llmproxy-subscription-agent
llmproxy-subscription-agent doctor --codex-bin /PATH/TO/CONTROLLED/codex
llmproxy-subscription-agent serve --codex-bin /PATH/TO/CONTROLLED/codex \
  --state-dir /YOUR/PRIVATE/agent-state --listen 127.0.0.1:8787 \
  --key-file /YOUR/PRIVATE/local-key --models YOUR_AUTHORIZED_MODEL
```

verify_runtime.py 只使用隔离目录和合成原生登录数据，连接固定本机 HTTPS 上游，使用临时测试 CA；检查原生认证头、完整历史／工具 ID／usage、执行 RPC 拒绝、取消、EOF／强制退出和正文文件扫描。它不替代真实账号授权／旋转刷新或跨地域验收。实时验收记录与未完成事项见 docs/31。
