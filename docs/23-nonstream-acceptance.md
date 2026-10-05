# 非流式转换验收

状态日期：2026-10-05。范围为已声明的 Chat、Responses、Messages、Gemini 协议类型，经整体 IR 转发。协议覆盖、模拟 HTTP 验收和真实模型能力分别记录；短文本请求通过不代表音视频、工具或缓存命中已经完成真实验收。

## 处理规则

| 类型 | 行为 |
| --- | --- |
| 有公共语义及目标字段 | 从来源 struct 解码到 IR，再构造目标 struct |
| 目标只能表达部分语义 | 保留可表达内容，并输出包含字段路径、来源与目标协议的警告 |
| 私有资源、强制约束或无法可靠识别的数据 | 明确拒绝；请求返回 422，完成响应转换失败返回 502 |
| 同协议且保留来源副本 | 保持原始专属字段、null、缺失及扩展；已编辑字段必须生效或报错 |
| 未知扩展 | 同协议保留；跨协议逐字段告警，不将字段值写入转换日志 |

HTTP 序列化位于 Gateway 和控制台边界，`ProtocolCodec` 始终使用协议 struct 与 IR。叶子级工具参数、结果、Schema 和未知扩展仍允许使用 `Value`。

## 请求字段覆盖

以下按声明字段分组记录公共映射和当前处理规则；子对象还需经过目标能力与约束检查。

| 协议 | 进入公共 IR | 专属或尚无公共映射：告警丢弃 | 无法跨 Provider 转移：拒绝 |
| --- | --- | --- | --- |
| Chat | messages、model、frequency_penalty、logprobs、max_completion_tokens/max_tokens、n、parallel_tool_calls、presence_penalty、prompt_cache_key/options/retention、reasoning_effort、response_format、seed、stop、stream、temperature、tool_choice、tools、top_logprobs、top_p、web_search_options | audio、function_call、functions、logit_bias、metadata、modalities、moderation、prediction、safety_identifier、service_tier、store、stream_options、user、verbosity、extra | 私有文件 ID；目标不能满足的强制工具选择、搜索域名限制和 Schema 约束 |
| Responses | input、instructions、max_output_tokens、model、parallel_tool_calls、prompt_cache_key/options/retention、reasoning、stream、temperature、text.format、tool_choice、tools、top_p | access_programs、context_management、include、max_tool_calls、metadata、moderation、safety_identifier、service_tier、store、stream_options、top_logprobs、truncation、user、extra；reasoning/text 子对象未映射字段逐项诊断 | background=true、conversation、previous_response_id、prompt、item_reference、私有文件库或容器状态；未完成函数调用 |
| Messages | max_tokens、messages、model、cache_control、output_config、stop_sequences、stream、system、temperature、thinking、tool_choice、tools、top_k、top_p | diagnostics、inference_geo、metadata、service_tier、extra；声明或子对象未映射字段逐项诊断 | container、container_upload、Provider 私有文件；非法缓存断点/TTL；目标无法表达的强制约束 |
| Gemini | contents、tools、tool_config、system_instruction、generation_config 中采样/输出/推理字段、cached_content 的引用信息 | safety_settings、labels、service_tier、store、extra；生成配置中的语音/图像/翻译/转录等专属字段；无公共能力的工具声明 | cached_content 实际引用、私有 Files API URL；目标无法保证的工具或搜索限制 |

说明：`background=false` 为同步生成，不再记录无意义的丢弃警告。后台生成依赖来源 Provider 的取回和取消接口，跨协议拒绝，同协议保留。Gemini 的模型由 HTTP 路径承载。生成参数的静态范围校验已有实现；各具体模型支持哪些参数仍由 Provider 返回结果验证，不在路由中猜测。

## 内容、响应与统计覆盖

| 内容 | 当前实现和验收 |
| --- | --- |
| 角色与指令 | system/developer/instructions 保留指令语义，映射为目标的系统入口；不降为 user。工具调用和结果按 ID、名称及顺序配对，缺少 ID 时显式补号并警告 |
| 文本与函数 | 四协议文本、函数声明、调用、结果和工具选择已有适配；HTTP 矩阵检查工具响应不变成普通文本，调用 ID 和工具名保持 |
| 图片与 PDF | URL、Data URL、内联 Base64 按目标能力映射；四协议单元及有效样本 HTTP 矩阵；Chat PDF 构造带 MIME 的 Data URL，非 PDF 拒绝；私有资源引用拒绝 |
| 文本文档 | Messages 保持 document；其他协议保留正文为文本，并告警标题、文档边界及引用能力损失 |
| 输入音视频 | Chat/Gemini 支持的内联音频映射；Gemini 视频载体保留；目标没有原生载体或 MIME 不支持时拒绝 |
| 输出媒体 | Gemini 的媒体输出进入 IR；目标没有等价输出块时拒绝。Chat audio 响应缺少请求中的格式上下文，跨协议明确拒绝，同协议完整保留；不猜 MIME、不静默丢音频 |
| 服务端工具与输出 | 搜索/代码能力单独建模；可见结果降为文本并告警；私有执行环境、文件和加密内容不伪造；其他专属能力按文档 22 的规则处理 |
| 状态与候选 | 截断、过滤、拒绝、无内容、候选边界和多候选选择规则已有回归；Gemini 缺失候选 index 按数组顺序补号 |
| 生成失败 | `Response.failure` 保存错误代码和说明，`Status::Cancelled` 区分取消。Responses 规范化编码保持 failed/cancelled，失败说明使用通用文本；其他正常完成外壳不能表达时返回独立失败，由 Gateway 返回 502 |
| 执行中 | queued/in_progress 不伪装为完成；跨协议完成编码拒绝。控制台明确显示后台轮询不支持；失败或取消不进入正常对话历史 |
| 总用量 | 输入/输出/总量、缓存读写、TTL、推理及目标可表达模态细分；核对总数、缓存读写与未缓存输入；缺失不补零 |
| 缓存/工具模态 | `CacheUsage.read_details` 和 `InputTokenDetails.tool_details` 使用共用的 `ModalityTokenDetails`；Gemini 完整读写；其他目标告警丢弃细分，保留总数；重复模态未编辑时保持原貌，编辑时合并，清空时移除；累计溢出拒绝 |
| HTTP 边界 | 路径、模型位置、目标鉴权、正文序列化、错误状态、ETag/Content-Length、请求前缀回放已有回归；跨协议非流式在正文完成前不发送真实响应头 |
| 大正文/取消 | 8 MiB 上限，已知长度超限提前 413，chunked 逐块累计；客户端断开时使用 Pingora idle 读取终止子请求并关闭上游；已有真实 TCP 取消测试 |

## 验收矩阵

行是客户端，列是 Provider。每格包含请求正向转换与响应反向转换。

| 客户端 \ Provider | Chat | Responses | Messages | Gemini |
| --- | --- | --- | --- | --- |
| Chat | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过，签名状态恢复 |
| Responses | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过，签名状态恢复 |
| Messages | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过，签名状态恢复 |
| Gemini | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过 | 文本与工具双回合通过 |

- 模拟 Provider：64 次基础 HTTP 请求（16 组合 × 文本/工具调用/工具结果/429），全部通过；另覆盖 9 次 failed/cancelled/queued 跨协议请求、超限拒绝和取消连接。检查路径、鉴权、模型、工具声明、具名选择、调用参数、工具结果、输出、总用量和缓存读取。
- 真实 Provider：DeepSeek / `deepseek-flash` 提供 Chat、Responses、Messages 接口；Gemini / `gemini-3.5-flash-lite` 提供 Gemini 接口。16 个文本与 16 个自动工具双回合均已通过。DeepSeek 工具双回合显式关闭思考模式；Gemini 同协议先修复历史重建签名丢失，另外 3 个跨协议方向通过短期状态修复后分别复验成功。
- DeepSeek 工具请求实际报告缓存读取 128 tokens，跨协议计数保留；Gemini 未报告缓存读取，保持缺失。缓存写入和 TTL 仍未完成真实验收。三个接口使用同一家 DeepSeek，不能推断所有协议厂商的专属能力相同。
- 原 PostgreSQL 仅查询；临时 SQLite 目录权限 0700，真实凭据沿用用户主密钥加密，路由副本测试结束删除。

### 工具回合的真实限制

- Gemini 3 的函数调用必须在后续历史的原始片段携带 `thoughtSignature`。已修复 Gemini 同协议移除来源副本、重建请求时丢失签名的问题，并覆盖并行调用签名位置。Chat、Responses、Messages 没有可携带该签名的标准函数字段；最初跨协议工具回传均返回 400，现由下述共享网关状态保存、恢复原片段完成工具回合。纯 `ProtocolCodec` 仍不保存跨请求状态，其他接入方需要自行承担该职责。[Gemini 签名要求](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)
- DeepSeek 默认思考模式不接受 `required` 或具名强制调用；请求会返回 400。基础互通使用自动选择，双回合验收通过客户端协议字段显式关闭思考；生产转换不擅自关闭思考。Gemini 的允许列表映射至 Chat/Responses 后，DeepSeek 兼容接口也可能不接受该扩展形式。这是模型/接口能力限制，不能通过静默放松工具约束掩盖。[DeepSeek 工具选择](https://api-docs.deepseek.com/api/create-chat-completion/)

### OpenObserve 实际读回

- Gateway 新增非流式 `event_kind=usage`，在目标协议编码前记录 Provider 的输入、输出、总量、缓存读写及已报告推理/工具计数；缺失不填零。超过同协议缓冲上限、压缩透传或流式响应暂不在该事件范围内。
- 正文转换在本次请求 span 内同步执行，`request`、`conversion`、`usage` 事件可按同一 trace 查询。
- 本地 OpenObserve 已读回日志、对应 trace 及 `llmproxy_requests` 指标。模拟 Messages→Chat 的计数为输入 10、输出 3、总量 13、缓存读取 4、缓存写入 2，其中 5 分钟与 1 小时分别为 1；请求指标为 1。记录中没有提示词、扩展字段值或 Provider 密钥。这验证遥测管线，不代表真实模型发生缓存写入。
- 项目 `.env` 的 OTLP 写入凭据有效：实际写入返回 200，日志已读回。该凭据不能用于查询 API，查询返回 401 属于凭据用途不同；此前“密码配置错误”的判断已纠正。验收使用独立查询认证，不修改 OTLP 写入配置。[OpenObserve 写入 token](https://openobserve.ai/opentelemetry/collector/)

### Gemini 跨协议工具状态方案

现使用既有 PostgreSQL／SQLite 数据库和主密钥保存状态，不改变协议 codec 的纯转换职责。单进程内存方案已替换，多个 Gateway 必须连接同一数据库并使用同一主密钥及相同路由作用域。

1. Gemini 响应含带签名的函数调用时，保存该轮的原始类型化函数调用片段，包括无签名的并行兄弟调用。客户端工具调用 ID 替换为随机状态引用，长度不超过 Chat 的 64 字符限制；签名不写入客户端正文或日志。
2. 客户端按原 ID 回传工具调用历史和结果。IR 正常转换成 Gemini 请求 struct 后，网关检查原工具名称、参数、同轮数量与顺序，再恢复原调用片段和 Provider 调用 ID。结果 ID 同时还原；并行结果允许乱序完成，在发送 Gemini 前按原调用顺序排列，保持无 ID 时的对应关系。
3. 状态绑定 Provider 地址、路径、TLS、凭据、上游模型、客户端协议、路由和客户端鉴权头的 SHA-256 摘要。修改调用内容、跨作用域使用、丢失/过期引用或改变并行调用组均返回 422，不补造签名。
4. 状态有效期为最后一次成功验证并续期后 24 小时，整个数据库最多 4096 个工具调用，密文预算 16 MiB；容量检查和整轮保存使用同一事务，不驱逐仍有效的状态。成功写入时清理过期记录。容量不足返回 502；无需后台清理任务或新增连接池。
5. 同步响应回调只暂存原始类型化片段；父请求在真实响应头发送前完成加密保存。请求正文完成后异步批量读取，验证名称、参数和并行组，再续期并发送上游。数据库故障返回 503、存储超时返回 504，客户端不会收到尚未保存的工具 ID。
6. 密文关联数据绑定状态引用和路由作用域，数据库行之间交换密文不能通过解密。读取、保存和续期都校验主密钥；明文片段、鉴权摘要和签名不进入控制台或日志。数据库迁移分别为 PostgreSQL `0016_tool_continuations.sql`、SQLite `0046_tool_continuations.sql`。

- [x] 实现隔离、过期、容量和并行组约束的状态存储。
- [x] 接入响应保存与请求恢复，协议转换仍使用 struct → IR → struct。
- [x] 模拟三种客户端的工具双回合与无效状态拒绝；验证缺失 Provider ID、并行结果乱序、参数篡改、签名不泄露。
- [x] 真实 Chat/Responses/Messages → Gemini 工具双回合复验，均返回成功并收到工具结果生成的最终回答。
- [x] 共享加密存储、事务容量检查、24 小时闲置续期；跨实例、重启、过期拒绝和发头前存储故障回归。
- [x] SQLite 与独立 PostgreSQL schema 验证跨连接读写、并发保存、作用域和错误主密钥拒绝。

可重复运行入口（工作区根目录，凭据从 `.env` 读取，不放在命令行中）：

```sh
rtk cargo test -p llmproxy-gateway --test nonstream_matrix
rtk proxy env LLMPROXY_LIVE_DATABASE_ENV=LLMPROXY_DATABASE_URL cargo test -p llmproxy-gateway --test configured_providers configured_provider_nonstream_matrix -- --ignored --nocapture
rtk proxy cargo test -p llmproxy-gateway --test configured_providers configured_openobserve_nonstream -- --ignored --nocapture
```

省略 `LLMPROXY_LIVE_DATABASE_ENV` 默认使用 `LLMPROXY_TEST_DATABASE_URL`；本地 `llmproxy_test` 无配置表，本次显式使用已配置的 `llmproxy_dev`。ignored 测试不会在日常回归中自动消费 token。

可设置 `LLMPROXY_LIVE_PAIR=openai_chat:gemini` 等已知协议组合，仅复验一个方向；测试仍包含文本、函数调用和工具结果回传，不跳过失败回合。

OpenObserve 测试保留 `.env` 中的 OTLP 写入认证；查询认证需通过 `LLMPROXY_OPENOBSERVE_QUERY_AUTHORIZATION` 单独注入登录凭据或有查询权限的凭据，不在命令行中展开或写入仓库。

### 媒体、结构化输出与缓存写入专项

| 能力 | 模拟 HTTP 边界 | 真实已配置 Gemini 模型 |
| --- | --- | --- |
| JSON Schema | 四客户端 × 四目标，检查目标原生 Schema 字段 | 四客户端均返回符合 Schema 的 `{"answer":"OK"}` |
| 图片 | 四客户端 × 四目标，核对 PNG 字节与 MIME | 四客户端均识别蓝色图片 |
| PDF | 四客户端 × 四目标，核对 PDF 字节、MIME 与 Data URL | 四客户端均读出页面中的 HELLO |
| 音频 | Chat/Gemini 的四个原生可表达组合；其他目标 422 且无完整上游请求 | Chat→Gemini、Gemini→Gemini 均转写 Hello |
| 视频 | Gemini 同协议保留字节；其他目标 422 且无完整上游请求 | Gemini 识别视频中的蓝色 |
| 缓存写入与 TTL 统计 | Messages 响应向四客户端投影；遥测在目标投影前保留缓存写入 3、短 TTL 1、长 TTL 2；Gemini 正文缺少写入字段时保持缺失 | 原厂 Messages 缓存创建与命中尚未联调 |

真实专项检查回答内容及非零输入/输出用量，不把 HTTP 200 当作成功。第一批的静音音频没有满足预期，后续音视频遇到 429；改用实际语音样本，并分别复验音频和视频后通过。未复用失败结果或把限流计为转换成功。

当前配置库启用的模型来自 DeepSeek、GLM、OpenRouter/free 和 Gemini，没有原厂 Claude 模型。使用现有 OpenRouter Provider 的临时 `anthropic/claude-haiku-4.5` 路由测试，两种 TTL 均返回 403，原厂 Messages 缓存创建及显式 TTL 仍需可用模型或权限。协议声明与模拟验收不受该配置缺口影响。

样本位于 `crates/llmproxy-gateway/tests/nonstream/assets/`：合成蓝色 PNG、含 HELLO 的 PDF、Hello 语音 WAV、一秒蓝色视频 MP4；不含用户数据。

```sh
rtk cargo test -p llmproxy-gateway --test nonstream_capabilities
rtk proxy env LLMPROXY_LIVE_DATABASE_ENV=LLMPROXY_DATABASE_URL LLMPROXY_LIVE_CAPABILITY=image cargo test -p llmproxy-gateway --test configured_providers configured_gemini_nonstream_capabilities -- --ignored --nocapture
```

`LLMPROXY_LIVE_CAPABILITY` 可选 `structured`、`image`、`pdf`、`audio`、`video`；可同时指定 `LLMPROXY_LIVE_PAIR`，目标须为 `gemini`。未知筛选条件或没有原生能力的组合会失败，避免空跑误报通过。各项分别复验可减少短时请求触发限流。

### 控制台浏览器验收

在独立 SQLite 和模拟 Gemini Provider 上，四种客户端均通过页面发送非流式请求，显示 OK 和「输入 10 · 输出 3 · 总计 13 · 缓存读取 4」。切换协议创建新会话，旧会话仍可见。

页面增加「停止生成」：停止通知先于发送发生时仍被记住；HTTP future 被释放后，跨协议网关关闭上游连接。实际浏览器验证了 Chat→Gemini 的等待中停止、继续同一会话以及 429 失败显示。停止轮次使用 Cancelled 状态，不记录用量，停止说明和失败回复不进入下一轮模型历史。停止 procedure 已注册并覆盖 CSRF 回归；多模态和工具的页面原生展示仍未验收。

## 仍开放的验收项

- [x] 专属参数与嵌套子对象的字段边界夹具，覆盖外层显式 null、嵌套扩展、usage 叶子、准确告警路径及强制约束拒绝；诊断不包含字段值。
- [x] 当前配置 Gemini 的 Schema、图片/PDF、语音、视频输入专项，以及四协议有效媒体和 Schema 的 HTTP 边界。
- [x] 缓存写入、TTL 计数的四客户端投影与 OpenObserve 实际读回；不等同真实模型缓存创建。
- [x] Gemini 努力等级和数字推理预算的真实联调；HTTP 层对无法换算预算的目标告警，不补造等级。
- [ ] 其他原厂专属工具、媒体输出、真实缓存写入与 TTL 的专项联调。
- [x] 浏览器中的四协议文本非流式对话与逐轮 usage/cache；Chat→Gemini 停止、恢复与失败显示。
- [x] 工具和多模态的页面原生展示及浏览器验收：四客户端纯工具调用、Gemini 图片/音频/视频、服务端代码与结果、Chat 未知格式音频下载与转录；不新增客户端工具执行器。
- [x] OpenObserve 中的请求关联、用量/缓存日志、对应 trace 与请求指标读回；模拟 Provider 不消费 token。
- [x] 跨协议流式基础链路：事件 IR、四协议状态机、逐帧 SSE、签名组持久化、安全错误、usage/cache、取消/背压及 16 方向真实/页面矩阵；专项外部门槛见[文档 24](24-protocol-completion-tasks.md)。

非流式原厂专属能力的门槛尚未全部关闭，不能将文本矩阵通过表述为全部能力验收。跨协议流式基础链路已可用，文档 24 的 S7-4 OpenObserve 流式实际读回已通过；S7-5 音频/转录展示、播放、清理及两协议附件实际保存通过，S7 已关闭。专项端到端验收追踪为 S8-4，原厂能力门槛追踪为 S8-5 与 N2/N3。

## 收尾任务追加验证

逐项进度与流式前置依赖见[四协议转换收尾任务](24-protocol-completion-tasks.md)。

- 服务端执行输出的模拟 HTTP 矩阵为 Responses/Messages/Gemini Provider × 四客户端，12 组合通过，可见结果保留，专属 ID 不跨协议发送，也不变成客户端函数调用。
- Gemini 图片输出和 Chat 音频输出的 8 组合验证：同协议保留实际载体，无等价输出载体的客户端得到 502；Provider 的 ETag 和私有音频 ID 不进入转换失败响应。
- 真实 `gemini-3.5-flash-lite`：Responses/Messages/Gemini 三种客户端代码执行均得到执行记录和 1073；四客户端推理等级转换均得到 1073 和非零推理词元。Messages→Gemini 曾返回 400，修正 adaptive + effort 的冲突参数后复验成功。Gemini 等级与明确预算无法同时表达时拒绝，不静默移除预算。[Gemini 推理配置](https://ai.google.dev/api/generate-content#ThinkingConfig)
- 原厂缓存创建测试入口已实现，分别检查 5m/1h 创建桶和重复前缀命中；现有 OpenRouter Provider 临时指定 Claude Haiku 4.5，两种 TTL 均返回 403，保持未验收。入口不验证等待 TTL 到期后的回收行为。[Claude 缓存规则](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- `gemini-2.5-flash` 的数字预算真实验收通过：Messages/Gemini 客户端均使用 1024 预算、2048 输出上限，实际推理词元分别为 340、367。8 个模拟 HTTP 组合检查 Messages/Gemini 原生预算字段，以及 Chat/Responses 的丢弃告警，不把数字预算换算成努力等级。
- 原生图片输出入口 `configured_gemini_image_output` 已实现，核对实际 Base64 图片载体、MIME/文件头与非零 usage。旧 `gemini-2.5-flash-image` 和当前 `gemini-3.1-flash-lite-image` 均返回 429，错误信息明确表示模型配额为零，未计为通过。

```sh
rtk proxy env LLMPROXY_LIVE_DATABASE_ENV=LLMPROXY_DATABASE_URL LLMPROXY_LIVE_CAPABILITY=code cargo test -p llmproxy-gateway --test configured_providers configured_gemini_native_capabilities -- --ignored --nocapture
rtk proxy env LLMPROXY_LIVE_DATABASE_ENV=LLMPROXY_DATABASE_URL LLMPROXY_LIVE_CAPABILITY=reasoning cargo test -p llmproxy-gateway --test configured_providers configured_gemini_native_capabilities -- --ignored --nocapture
rtk proxy env LLMPROXY_LIVE_DATABASE_ENV=LLMPROXY_DATABASE_URL cargo test -p llmproxy-gateway --test configured_providers configured_messages_cache_creation_and_hit -- --ignored --nocapture
```

真实专项可用 `LLMPROXY_LIVE_MODEL_ID_OPENAI_CHAT`、`LLMPROXY_LIVE_MODEL_ID_OPENAI_RESPONSES`、`LLMPROXY_LIVE_MODEL_ID_ANTHROPIC_MESSAGES`、`LLMPROXY_LIVE_MODEL_ID_GEMINI` 指定配置库中已启用且具有对应协议的模型 ID；没有命中时明确失败，不空跑。

配合模型 ID 可设置 `LLMPROXY_LIVE_UPSTREAM_MODEL_<协议名大写>`，只覆盖临时 SQLite 的模型名，不修改 PostgreSQL。数字预算专项需显式设置 `LLMPROXY_LIVE_CAPABILITY=budget` 并选择支持预算的模型；图片输出入口同样需要显式选择图片模型。

本次收尾的完整 workspace 验证为 248 项通过；追加数字预算用例所在的 5 项 HTTP 专项通过。workspace Clippy、追加 Gateway 测试 Clippy、格式和 diff 检查通过。外部验收入口默认忽略，缓存 403 与图片零配额 429 保持未通过。以下 236 项记录是前一轮验收，保留其历史结果。

本轮验证：`cargo test --workspace --offline` 236 项通过、4 项外部验收测试默认忽略（24 个测试套件）；`cargo clippy --workspace --all-targets --offline -- -D warnings`、格式检查及 diff 检查通过。Gemini 三个跨协议方向的文本与工具双回合、Schema 和有效媒体输入真实专项通过；SQLite／独立 PostgreSQL schema 的持久化验收通过。OpenObserve 使用原写入配置与独立查询认证，日志／trace／指标及缓存写入细分读回通过；四客户端文本浏览器验收、跨协议取消后关闭上游和恢复对话通过。

参考：[Responses 后台生成](https://developers.openai.com/api/docs/guides/background)、[Chat API](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Gemini UsageMetadata](https://ai.google.dev/api/generate-content#UsageMetadata)。
