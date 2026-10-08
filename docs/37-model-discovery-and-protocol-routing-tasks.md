# 模型发现与跨协议路由任务

设计：[模型发现与跨协议路由](36-model-discovery-and-protocol-routing.md)。

- [x] 确认接口字段、对外路径、协议回退和停用语义。
- [x] 在快照中实现共用跨协议解析及目录生成，验证优先级和禁用保护。
- [x] 将真实目标的探活状态加载到网关快照，验证明确不可用过滤。
- [x] 接入 GET /models 与 /v1/models，并复用解析器处理实际推理请求。
- [x] 集成验证四种协议的桥接、模型路由、目录去重与健康状态刷新。
- [x] 执行完整测试及 Clippy，记录验证结果。

## 已完成验证

- 快照解析单元测试覆盖同协议优先、固定顺序回退、明确停用保护、全部不可用、目录排序和去重。
- model_discovery 三项集成测试通过：模型别名与路由名称自动桥接、16 个非流式与 16 个流式方向、目录契约、空目录、405、探活过滤和恢复。流式日志断言确认每次请求只统计一次。
- Clippy workspace/all-targets（-D warnings）通过；本次改动文件 rustfmt 检查与 git diff --check 通过。
- 首次并行完整测试遇到 Chat 历史保存等待超时；该测试单独重跑通过。随后完整串行验证 `cargo test --workspace -- --test-threads=1` 通过：391 passed、12 ignored、36 suites。
