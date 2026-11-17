# SSE 代理审查与推荐配置

本次检查覆盖反向代理、正向 HTTP 代理、HTTP relay，以及 HTTPS CONNECT 的边界。修改后，反向代理与明文 HTTP 正向/relay 路径可以逐帧转发 SSE，并继续复用已完整结束响应的空闲连接。下面是配置起点，不是对所有网络都适用的唯一最优值。

## 审查结果与修复

| 原有行为 | 影响 | 当前处理 |
| --- | --- | --- |
| 反向代理已对 `text/event-stream` 使用流式 body | 基本 SSE 转发可用 | 保留按下游需求读取的流式转发，不创建无界事件队列 |
| 正向 HTTP 和 HTTP relay 完整读取响应后才返回 | 无限 SSE 无法及时返回响应/事件 | 直接传递流式 body；relay 改由 Hyper 解析 HTTP framing |
| 默认 300 秒硬连接寿命，无禁用语义 | 活跃 SSE 到期也会断开 | `max_connection_lifetime_secs: 0` 显式禁用硬截止；未配置仍为 300 秒 |
| 只有新建 TCP connect timeout | 复用到半失效连接后，等待响应头可能一直挂起 | 独立响应头 deadline，超时关闭该请求并返回 504 |
| SSE 流没有静默检测 | 黑洞连接可能长期占用资源 | 可配置流空闲 deadline，超时结束 body 并释放上游；已发头后以断流通知客户端 |
| SSE 透传 `Connection` / `Transfer-Encoding` 等 | 上下游连接语义和 framing 混淆 | 删除逐跳头和所有 `Connection` 指定头，由 Hyper 生成本跳 framing |
| SSE 类型检测用大小写敏感的子串匹配 | 合法类型漏检、相似非法类型误检 | 忽略媒体类型大小写，准确匹配分号前的 MIME 类型 |
| least-connections 计数在响应头到达后释放 | 长流未计入后端负载 | 计数跟随 SSE body，EOF、错误或取消后释放 |

SSE 响应附加 `X-Accel-Buffering: no`，供支持该头的前置代理使用；保留上游 `Cache-Control`、事件数据、`id:`、`retry:`、状态码及普通业务头。请求中的查询参数、认证头及 `Last-Event-ID` 按正常代理规则转发。代理不解析或重写事件，也不注入心跳、不自动续传断流、不在响应开始后重试。

## 推荐：反向代理

直接使用 [config_reverse_sse.json](../examples/config_reverse_sse.json)：

```bash
cargo run --release -- --config examples/config_reverse_sse.json
curl -N http://127.0.0.1:8888/events
```

示例后端是 `http://127.0.0.1:3000`，按部署修改。要连接 HTTPS 后端，将 target 改为有效的 `https://...` 地址。监听公网或终止 TLS 的配置见 [HTTPS 配置](https-setup.md)。

| 配置 | 示例值 | 含义与调节方法 |
| --- | --- | --- |
| `connect_timeout_secs` | 10 | 新建上游 TCP 连接的超时；不能替代 TLS/响应头阶段的 deadline |
| `max_connection_lifetime_secs` | 0 | 禁用客户端连接/隧道硬寿命，允许 SSE 持续；非零值仍会切断活跃流 |
| `reverse_proxy_config.pool_max_idle_per_host` | 32 | 每个 route client、每个 host 最多保留的空闲连接数；不是活跃流并发上限 |
| `reverse_proxy_config.pool_idle_timeout_secs` | 15 | 淘汰空闲连接，选择小于上游/LB/NAT 已知最短空闲超时的值，留足余量；不限制活跃 SSE |
| `reverse_proxy_config.response_header_timeout_secs` | 30 | 从提交上游请求到收到响应头的总等待上限，包括建连和上传等待；0 禁用。LLM 如长时间延迟响应头，可改为 120，代价是坏连接检测更慢 |
| `reverse_proxy_config.stream_idle_timeout_secs` | 60 | 等待下一个上游 SSE 非空 data frame 的静默上限；0 禁用。注释心跳也是 data，可刷新 deadline |
| `max_connections` | 2000 | 同时接入的客户端连接上限；按文件描述符、内存与后端容量调整，HTTP/1 每条活跃流通常还占一个上游 socket |

以上两个新 timeout 的默认值分别为 30 和 60 秒，适用于所有使用对应配置的反向 route。route 自带的 `reverse_proxy_config` 会整体替代全局对象，缺省字段使用结构体默认值，**不会逐字段继承全局值**；给 SSE 单独配置 route 时应填写需要保留的全部参数。普通非 SSE 的反向响应仍使用既有缓冲路径，流空闲超时仅作用于 SSE。

上游建议每 15 秒发送并 flush 一次 `: ping\n\n`，启动后也应尽快发送响应头；这是部署建议，代理不会替上游产生心跳。如果业务允许超过 60 秒没有任何字节，应提高流空闲超时，或设为 0 并由业务层处理失活。客户端重连后带上 `Last-Event-ID`，由后端实现事件留存与去重。对产生任务/扣费的 POST 流式请求，默认保持不配置重试；需要重试时由业务定义幂等键和恢复语义。

若前面还有 Nginx，可在对应 location 使用：

```nginx
proxy_http_version 1.1;
proxy_set_header Connection "";
proxy_buffering off;
proxy_cache off;
proxy_read_timeout 90s;
```

此处假设上游每 15 秒有心跳，`proxy_read_timeout` 是两次读取间的超时；同时核对 CDN/LB 的实际 idle timeout 和最大响应时长。压缩如不能逐事件 flush，也会延迟事件；该代理保持压缩字节透明传递，不替后端解压/重压缩。

## 正向代理与 relay

使用 [config_forward_sse.json](../examples/config_forward_sse.json)：

```bash
cargo run --release -- --config examples/config_forward_sse.json
curl --noproxy '' -N -x http://127.0.0.1:8889 http://127.0.0.1:3000/events
```

`forward_proxy_config.response_header_timeout_secs` / `stream_idle_timeout_secs` 的默认值同样为 30/60，0 禁用；这些参数覆盖正向 HTTP 直连及 HTTP relay 响应。`connection_pool_enabled: true` 保留直连上游连接池，`idle_timeout_secs: 15` 控制池空闲淘汰。HTTP relay 当前每个请求新建连接，不提供跨 relay 请求的连接池。

**HTTPS 目标通常走 CONNECT**：代理转发的是不透明 TLS 字节，无法识别 SSE、事件心跳或 `Last-Event-ID`，上述 HTTP 响应头/流空闲 deadline 不适用于隧道内部。`max_connection_lifetime_secs: 0` 同样取消 CONNECT 隧道的硬寿命，TLS/HTTP 超时与 SSE 恢复由隧道两端负责。

## 连接复用与能力边界

- 服务端当前使用 Hyper HTTP/1 listener（包括 TLS listener），未提供客户端到 Bifrost 的 HTTP/2 多路复用；不能因为 client builder 有 `http2_only(false)` 就认为已经验证了 HTTP/2。需要浏览器 HTTP/2 接入时，可在前面使用支持 HTTP/2 的网关。
- HTTP/1 活跃 SSE 占用独立上游连接，只有响应完整结束且连接仍可复用时才回池；取消/异常/超时会释放 body，不能把未读完的响应作为空闲连接重用。
- 空闲淘汰、TCP keepalive 和 Hyper 的失效连接处理可降低 stale 风险，但不能消除“借出后立即失效”的竞态；响应头 deadline 限制其占用时间。未增加会重复提交业务请求的无条件重试。
- 流转发由下游消费驱动，保留 backpressure。流 idle deadline 在 body 被轮询时检测；下游完全不读取时，不保证在 deadline 到达的一刻主动关闭 socket，也不将它作为慢客户端写超时。
- 当前请求耗时等已有指标主要记录到响应头阶段；SSE 目标的 inflight 计数已修复，但没有新增事件级监控或声称做过高并发压测。
- reload/shutdown 沿用现有 generation 生命周期，不保证跨重载保持 SSE；客户端仍应实现重连。不要把健康检查 endpoint 指向无限 SSE。

## 验证

`tests/sse_proxy_tests.rs` 使用真实本地 TCP socket 验证：首事件在上游 EOF 前到达、GET 查询与 `Last-Event-ID`、逐跳头清理、客户端断开传播、活跃流不阻塞其他请求、完整 SSE 的连接复用、空闲淘汰、已关闭连接恢复、复用连接的无响应超时、流静默超时、禁用硬寿命/idle，以及正向和两级 relay 的流式转发。`src/streaming.rs` 的单元测试补充心跳续期与 EOF/错误/取消后的计数释放。没有将这些功能测试当作性能或所有生产网络条件的证明。

协议依据：[HTTP/1.1 连接管理](https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3)、[逐跳头处理](https://www.rfc-editor.org/rfc/rfc9110.html#section-7.6.1)、[WHATWG SSE](https://html.spec.whatwg.org/multipage/server-sent-events.html)。
