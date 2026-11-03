# ADR-0002：句柄型微内核

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[总体架构](../architecture.md)、[执行模型](../execution.md)、[内存模型](../memory.md)

## 背景

HolyKernel 需要强隔离、最小可信基与可重启服务器。单体内核把驱动、文件系统与 POSIX 语义放入内核，扩大可信基并降低故障隔离。

## 决策

1. 内核只提供 **TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Untyped、IRQHandler**。
2. 所有资源通过 **Handle** 访问；Handle 携带 index、generation 与 rights（READ/WRITE/EXECUTE/TRANSFER/ADMIN）。
3. 每进程独立 **VSpace**；内核与用户态地址空间分离。
4. 驱动、VFS、文件系统、POSIX 兼容层、显示服务运行在用户态。
5. Root Server 获得初始 Untyped/HandleTable/Endpoint，负责服务器启动与句柄分配。
6. 服务器崩溃由 Root Server 重启，旧句柄被关闭。
7. POSIX、VFS、文件系统、网络协议栈与设备类驱动位于用户态服务；内核提供机制。

## 后果

**收益**

- 故障隔离：驱动/文件系统/POSIX 崩溃影响局部，可重启。
- 最小可信基：内核对象与 IPC 可审计。
- 最小权限：服务器只获得职责所需的 Handle。
- 可验证：对象、句柄、IPC 状态机清晰。

**成本与取舍**

- IPC 与服务器切换带来性能开销；需要快速路径与共享 Frame。
- 服务器数量增加，启动与监督复杂。
- 调试跨服务器问题需要 trace 与确定性模拟。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 单体内核 | 性能直接；可信基大，故障隔离弱。 |
| 微内核 + 用户态驱动，采用简化句柄表 | 结构简单；权限与隔离弱。 |
| 句柄型微内核 | 隔离与最小权限强；实现复杂度高。 |

## 架构一致性检查

- 内核对象集合固定；新机制通过 ADR 加入。
- 服务器通过 Endpoint 与 Handle 协作。
- 全局命名空间与 POSIX 状态由用户态服务维护；内核提供机制。

## 相关

- [ADR-0005：Handle 与 IPC](0005-handles.md)
- [ADR-0003：POSIX personality](0003-posix-personality.md)
