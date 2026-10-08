# 模型发现与跨协议路由

## 接口约定

提供 GET `/models`，并将 `/v1/models` 注册为同一处理器的兼容别名。不新增 `/api/models`，不访问上游模型列表或触发探活。响应为 `object: list` 和 `data`。每项包含相同的 `id`、`name`（模型别名或模型路由名称），`object: model`、`owned_by: llmproxy`、`created: 0`（当前未保存目录创建时间，表示未知），以及 `protocols` 对象。对象键为 chat、responses、messages、gemini，值为网关对外推理路径。Gemini 返回 `/v1beta/models/{model}:generateContent` 模板，替换时对完整别名进行 URL 路径段编码；流式方法为 streamGenerateContent。

模型和路由统一合并、名称去重并按名称排序。名称直接用于推理请求。只列出实际解析到可调用目标的协议；无可调用协议则隐藏整个条目。空目录返回 200 和空数组。其他方法返回 405、Allow: GET。认证与现有对外推理入口保持一致。

响应示例：

```json
{
  "object": "list",
  "data": [{
    "id": "flash",
    "name": "flash",
    "object": "model",
    "created": 0,
    "owned_by": "llmproxy",
    "protocols": {
      "chat": "/v1/chat/completions",
      "responses": "/v1/responses",
      "messages": "/v1/messages",
      "gemini": "/v1beta/models/{model}:generateContent"
    }
  }]
}
```

## 共用目标解析

在现有不可变 ProviderSnapshot 上增加解析方法，列表与转发复用，不新增策略框架。优先入口协议的配置；若没有同协议配置或其模型探活确认无法调用，按照 Chat、Responses、Messages、Gemini 顺序寻找其他可调用目标。同协议配置明确停用时保留拒绝，避免桥接绕过停用。无配置返回 404；存在配置但都不能调用返回 503。

选定目标后保留其真实上游协议、路径、凭据、思考能力与上游模型 ID，继续复用既有请求、响应和 SSE 桥接。桥接表示协议接受能力，不保证模型支持所有多模态、工具等特性。模型路由仍按原目标顺序选取首个启用且协议匹配的目标，不增加路由内自动故障切换。

探活判断采用目标模型与真实上游协议的新鲜结果，Unavailable、连续三次失败 Unhealthy 和 Authentication 阻止调用；过期、未知、限流与单次失败允许尝试。快照刷新一并更新探活状态，约一秒生效；读取失败保留上一份快照。目录使用单次快照，避免响应中协议之间状态漂移。在途请求保留旧快照。

## 验证

单元测试覆盖同协议优先、固定顺序回退、停用保护、全部不可用、目录排序和去重。集成测试覆盖模型别名及路由名称的跨协议实际请求转换、目录路径和方法、探活不可用过滤与恢复。完整 workspace 测试和 Clippy 通过后更新任务清单。
