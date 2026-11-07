# ADR-0008：抢占式对称多处理

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[执行模型](../execution.md)、[Holy Cadence Model](../scheduling.md)、[ADR-0017](0017-holy-cadence.md)、[总体架构](../architecture.md)

## 背景

微内核需要可预测的执行切换、IPC 阻塞/唤醒与 SMP 支持。服务器、POSIX 线程与原生任务共享同一套执行机制。

## 决策

1. **抢占式执行**：中断返回、Grant 到期、IPC 阻塞触发切换。
2. **对称多处理**：所有 CPU 同等参与；BSP 启动 AP。
3. **每 CPU Beat**：类别队列、Grant、一次性定时器、`need_resched` 与 idle TCB。
4. **调度契约**：Task 持有 Cadence，TCB 在 Task 内轮转；策略由 [Holy Cadence Model](../scheduling.md) 与 ADR-0017 定义。
5. **亲和性**：`CpuMask` 限制可运行 CPU；`migratable` 决定迁移资格。
6. **负载均衡**：空闲 Beat 拉取可迁移 Task；迁移写入 Meter 与 trace。
7. **IPC 调度**：Endpoint 等待队列按 Cadence 类别与虚拟键值排序，同类别内 FIFO；Notification 唤醒进入 deadline 队列。
8. **故障隔离**：TCB 故障通过 Fault Endpoint 交给 POSIX Server；内核检测到保证失效时停机。

## 后果

**收益**

- 交互响应由 Cadence 的 Latency 类保障；IPC 阻塞/唤醒与调度自然配合。
- 多核利用率高；适合 QEMU/真机 SMP。
- POSIX 线程、服务器与原生任务共享同一调度模型。
- 调度策略集中在 Tempo Server，内核 Beat 保持小而确定。

**成本与取舍**

- 并发复杂度高：锁、竞态、IPI、lending 恢复与迁移。
- Realtime 准入需要 Tempo 的可调度性分析。
- 需要确定性模拟与压力测试。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 协作式调度 | 简单；死循环占住 CPU。 |
| 主从多核 | 实现直接；扩展性受限。 |
| 全局单队列 | 实现简单；SMP 锁竞争与缓存抖动。 |
| 每 CPU 独立策略 | 灵活；跨核公平与迁移难以保证。 |

## 架构一致性检查

- Task 是调度主体；TCB 是执行单元；同一 Task 的 TCB 共享 Cadence 与 Meter。
- IPC 阻塞/唤醒通过 Endpoint/Notification；`call/reply` 触发 lending frame。
- 中断上半部使用固定分配与自旋锁。
- Beat 只执行 Cadence；授予、调整与准入由 Tempo Server 完成。

## 相关

- [ADR-0002：微内核](0002-microkernel.md)
- [ADR-0005：Handle 与 IPC](0005-handles.md)
- [ADR-0017：Holy Cadence Model](0017-holy-cadence.md)
