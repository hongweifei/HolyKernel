# ADR-0017：Holy Cadence Model 作为原生调度契约

- 状态：**已接受**
- 日期：2026-11-07
- 关联：[Holy Cadence Model](../scheduling.md)、[执行模型](../execution.md)、[章程](../charter.md)、[ADR-0016](0016-hmp-native-contract.md)、[Handle 与 IPC](0005-handles.md)、[权限模型](../permissions.md)

## 背景

CPU 时间是 HolyKernel 最后一个缺少 Holy 原生对象面的核心资源。内存由 HMP 统一为 Capacity/Frame/Region/Ledger，文件由 HolyFS 统一为 DocId/Revision/Tag/Snapshot；调度的参数仍分散在 TCB、队列与常量中。

这带来四个结构性问题：

1. **主体错位。** Native Task 是 Holy 的一等对象，公平与预算却按 TCB 计算，一个进程开多线程即可放大份额。
2. **策略分散。** 调度参数散落在 TCB、队列与常量里，无法授予、收窄、撤销、审计。
3. **计费与借贷缺失。** 微内核 `call/reply` 让服务时间归属不清，调用方与服务器之间缺少显式借贷与计费机制。
4. **状态不可恢复。** 调度状态不在 Task 视图、checkpoint 与 Debug Server 中，任务管理器只能估算 CPU 百分比。

HMP 已经证明“统一对象面 + 铸造链 + 记账”能够收敛资源策略。调度采用同一方法。

## 决策

采用 **Holy Cadence Model（HCM）** 作为原生调度契约。

### 1. 原生对象

- `Task`：调度主体，持有 Cadence、Meter、TCB(s) 与父子关系。
- `Cadence`：时间契约 Handle，表达类别、权重、预算、周期、截止期、延迟目标、亲和性、lending 策略、父契约与 epoch。
- `Meter`：计量 Handle，记录消耗、等待、节流、超限、迁移、切换与虚拟进度。
- `Tempo Server`：用户态策略服务，负责授予、拆分、调整、准入与撤销。
- `Beat`：每 CPU 内核机制，负责类别队列、Grant、定时器、抢占、idle 与负载均衡。
- `Grant`：内核内部执行许可，作为时间片记账单位。

### 2. 铸造链

`Root Cadence 池 → Tempo 授予 → Cadence → Task → Grant → Meter`。每一步只收窄 Rights 与预算；Handle 转移是唯一授权方式；撤销通过 epoch 完成。

### 3. 两级调度

- Task 级：按类别与虚拟时间选择下一个 Grant。
- TCB 级：同一 Task 内的可运行 TCB 轮转分享 Grant。
- 公平、预算、计量都记在 Task 上。

### 4. 类别

| 类别 | 算法 | 用途 |
| --- | --- | --- |
| `Realtime` | EDF + CBS | 显示、音频、输入 |
| `Latency` | 带虚拟截止期的加权公平 | 终端、文件服务、窗口 |
| `Throughput` | 加权虚拟时间 | 编译、索引、后台任务 |
| `Idle` | idle TCB | 空闲 CPU |

选择顺序：`Realtime` → `Latency` → `Throughput` → `Idle`。

### 5. 预算与 Throttled

`budget > 0` 的 Cadence 在一个 `period` 内限制 CPU 时间；超额进入 `Throttled`，下一周期由 Beat 补充预算并重新入队。`budget = 0` 的 Cadence 按权重长期分享。

### 6. Lending

`call` 期间建立 lending frame：服务端取得调用方与服务端中更强的有效契约，执行时间记入调用方 Meter；`reply`、超时或退出时恢复。深度由双方 Cadence 的 `lending.max_depth` 限制，默认 2。服务端预算持续生效，调用方预算耗尽后进入 `Throttled`。

### 7. Tickless Beat

每 CPU 一个一次性定时器，编程到 Grant 结束、下一个虚拟截止期、周期补充、sleep 或 IPC 超时的最早者。全局 deadline 队列承载时间等待；空闲 CPU 运行 idle TCB 并执行 `hlt`。

### 8. 亲和性与负载均衡

Cadence 携带 `CpuMask` 与 `migratable`。唤醒优先选择亲和空闲 CPU；空闲 Beat 拉取可迁移 Task；迁移按权重负载与缓存热度决策并写入 Meter/trace。

### 9. 可观察与可恢复

`TaskInfo` 暴露 Cadence/Meter 快照；trace 覆盖入队、抢占、节流、借贷、迁移与超限；checkpoint 保存契约身份与计量水位，恢复时由 Tempo 重新准入。

### 10. ABI

进程与线程段保留 `SYS_CADENCE_INFO`、`SYS_CADENCE_UPDATE`、`SYS_CADENCE_SPLIT`、`SYS_CADENCE_REVOKE`、`SYS_METER_INFO`、`SYS_TASK_ATTACH_CADENCE`。`SYS_SLEEP` 进入 Beat 的 deadline 队列。原生调度 ABI 以 Cadence 为中心；POSIX `nice`/`sched_*` 由 POSIX Server 映射为 Cadence 权重与类别。

## 后果

**收益**

- CPU 时间成为与内存同级的显式资源，具备授予、拆分、撤销、审计与快照能力。
- Native Task 成为公平与预算主体，多线程任务按 Task 份额运行。
- 优先级反转与时间计费统一为 caller-pays lending。
- 调度策略集中在 Tempo，内核 Beat 保持小而确定。
- Tickless、idle TCB、亲和性与负载均衡统一在 Beat 内实现。
- 任务管理器、Debug Server 与 checkpoint 共享同一份 Cadence/Meter。

**成本与取舍**

- 新增 Cadence/Meter 对象、Rights、ABI 与 checkpoint 格式。
- Lending 引入跨 Task 的有效契约与恢复路径，需要覆盖超时、撤销与退出。
- Realtime 准入需要可调度性分析；Tempo 承担策略复杂度的主要部分。
- Meter 与 trace 增加热路径开销，需要在采样与环形缓冲上控制。
- 原生调度 ABI 以 Cadence 为中心，POSIX 语义在 POSIX Server 边界完成映射。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 保留集中式优先级队列 + 固定时间片 | 实现简单；缺少预算、公平、借贷、审计与恢复能力。 |
| 全局 CFS/EEVDF | 公平性好；策略隐式、按线程计算、缺少 Native Task 归属与显式契约。 |
| 只实现 Realtime/EDF | 实时性好；通用服务缺少份额与延迟层次。 |
| 调度策略全部放用户态 | 机制/策略分离彻底；每次调度 IPC 不可行，Beat 必须留在内核。 |
| Cadence 只做计量 | 实现简单；CPU 资源滥用、预算与准入缺少执行面。 |

## 架构一致性检查

- **TCB**：执行单元；归属 Task，共享 Task 的 Cadence 与 Meter。
- **Arena**：Task 持有内存根；Cadence 只描述时间资源。
- **HandleTable**：Cadence/Meter 使用统一 Handle、Rights 与 epoch 撤销。
- **Endpoint/Notification**：`call/reply` 是 lending 的触发点；等待队列按 Cadence 类别与虚拟键值排序。
- **HMP**：Cadence/Meter 与 Capacity/Ledger 同构，分别管理 CPU 时间与内存；两者都通过 Handle 授予。
- **Root/Tempo**：Root 决定初始授予；Tempo 决定普通服务策略；内核只执行契约。
- **Debug/Checkpoint**：通过 Handle 读取 Cadence/Meter；checkpoint 保存契约身份与水位。
- **确定性**：Hosted 虚拟时钟驱动 Beat；相同种子产生相同调度 trace。

## 相关

- [Holy Cadence Model](../scheduling.md)
- [执行模型](../execution.md)
- [章程](../charter.md)
- [Holy Memory Plane](../hmp.md)
- [Handle 与 IPC](0005-handles.md)
- [权限模型](../permissions.md)
