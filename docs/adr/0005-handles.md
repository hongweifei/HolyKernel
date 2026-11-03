# ADR-0005：Handle 与 IPC

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[执行模型](../execution.md)、[总体架构](../architecture.md)

## 背景

微内核需要显式授权、最小权限、句柄回收与跨服务器调用。全局对象命名与隐式权限会扩大攻击面。

## 决策

1. 资源通过 **Handle** 访问：`index + generation`；rights 为 `READ`、`WRITE`、`EXECUTE`、`TRANSFER`、`ADMIN`。
2. Handle 存放在 Native Task 的 **HandleTable**；TCB 引用同一张表；支持查找、duplicate、close、transfer。
3. `duplicate` 需要 `ADMIN`，结果 rights 只能收窄；`close` 使旧 generation 失效。
4. 每个对象有 **epoch**；owner 的 `revoke_all` 递增 epoch，使该对象所有旧 Handle 失效，O(1) 且无需遍历句柄表。
5. 每 Task/服务器有 **Handle 配额**；`duplicate`/`transfer` 前检查，耗尽返回 `NoHandles`。
6. `transfer` 需要 `TRANSFER`；接收方必须通过配额检查；发送方失去 Handle。
7. 没有 CNode、派生树、递归撤销或 badge 权限；回收通过 `close`、`revoke_all`、任务退出与服务器重启。
8. IPC 使用 **Endpoint**：send/recv/call/reply；消息携带 label、words 与 Handle。
9. 异步事件使用 **Notification**；中断下半部与 DMA 完成通过 Notification 交付。
10. 大消息使用共享 **Frame**；消息只传 Frame Handle。
11. 内核不维护全局对象命名；服务器通过 Endpoint 暴露接口。
12. 服务器重启时关闭旧 Handle，客户端重连新 Endpoint。

## 后果

**收益**

- 句柄表模型简单：查找、duplicate、close、transfer 四个核心操作。
- generation 防止 use-after-close；对象 epoch 提供 O(1) 撤销。
- 配额限制 Handle 泄漏与 DoS；审计可定位异常增长。
- 服务器重启关闭旧句柄，客户端重连新 Endpoint。
- IPC 类型统一；跨语言（Rust/C）映射清晰。
- 大块数据零拷贝通过 Frame。

**成本与取舍**

- 权限粒度中等，5 种 rights；复杂策略仍需服务端判断。
- 撤销是对象级；需要按对象重建 Handle。
- 配额需要调参与监控。
- IPC 性能需要快速路径与批处理。
- 客户端需要处理 Endpoint 断开与重连。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| CNode + 派生/撤销树 | 权限精细；并发与生命周期复杂。 |
| 文件描述符风格 | 应用熟悉；类型与权限信息弱。 |
| 服务级 Handle | 实现最简；权限逻辑散落到服务器。 |

## 架构一致性检查

- 内核对象通过 HandleTable 暴露；服务器通过 Endpoint 组合。
- 对象 epoch、配额、transfer 规则都通过 HandleTable 实现。
- POSIX fd 表映射到 VFS FileHandle；Handle 在 POSIX Server 与内核之间传递。
- 服务器重启关闭旧句柄并重新注册 Endpoint。

## 相关

- [ADR-0002：微内核](0002-microkernel.md)
- [ADR-0003：POSIX personality](0003-posix-personality.md)
