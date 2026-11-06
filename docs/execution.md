# 执行模型

状态：**设计中** · 上级：[总体架构](architecture.md)

本文描述 TCB、调度、Handle、Endpoint/Notification IPC 与故障处理。内核只提供线程与通信机制；POSIX 进程、线程、信号与 fd 由用户态 POSIX Server实现。

---

## 1. TCB 模型

```rust
pub struct Tcb {
    id: TcbId,
    state: AtomicU8,
    priority: Priority,
    affinity: CpuMask,
    context: UnsafeCell<Context>,
    kernel_stack: Stack,
    vspace: Handle<VSpace>,
    handle_table: Handle<HandleTable>,
    fault_endpoint: Option<Handle<Endpoint>>,
    ipc_state: IpcState,
    stats: TcbStats,
}
```

- **TCB** 是唯一的内核执行体。
- 每个 TCB 绑定一个 VSpace，并引用所属 Task 的 HandleTable。
- 线程入口、栈、TLS 由用户态服务器与 libc 配置。
- 内核以 TCB 为调度单位；POSIX 进程是共享 VSpace 的 TCB 集合。

### 状态机

```mermaid
stateDiagram-v2
    [*] --> Inactive
    Inactive --> Runnable: configure / start
    Runnable --> Running: dispatch
    Running --> Runnable: 时间片结束 / yield
    Running --> Blocked: IPC / 等待
    Blocked --> Runnable: IPC 到达 / 通知
    Running --> Inactive: exit / fault
    Inactive --> [*]
```

- **Inactive**：已创建，待配置或已退出。
- **Runnable**：在 run queue 中。
- **Running**：正在 CPU 上执行。
- **Blocked**：等待 Endpoint、Notification 或定时器。
- **Suspended**：由调试器或 Root Server 暂停。

---

## 2. 调度

- **抢占式**：中断返回、时间片耗尽、IPC 阻塞触发切换。
- **固定优先级 + 同优先级轮转**：32 个优先级，0 最高，31 为 idle。
- **每 CPU 一个 run queue**：优先级位图 + FIFO。
- **Tickless**：一次性定时器编程到“时间片结束/下一唤醒”的较早者。
- **SMP**：所有 CPU 同等参与；BSP 启动 AP。
- **亲和性**：`CpuMask` 限制可运行 CPU。
- **负载均衡**：idle CPU 拉取 Runnable TCB。

```mermaid
flowchart LR
    subgraph CPU0["CPU 0"]
        RQ0["RunQueue 0..31"] --> IDLE0["idle TCB"]
    end
    subgraph CPU1["CPU 1"]
        RQ1["RunQueue 0..31"] --> IDLE1["idle TCB"]
    end
    Wake["IPC / Notification / IPI"] --> RQ0
    Wake --> RQ1
    Balance["load balance"] --> RQ0
    Balance --> RQ1
```

- 默认时间片 4 ms。
- 上下文切换保存 callee-saved 寄存器、栈指针、FPU/向量状态。
- 目标：热切换 ≤ 1 µs（QEMU 基准）。

---

## 3. Handle

```rust
pub struct Handle {
    index: u32,
    generation: u32,
}

pub struct Rights(u32); // READ | WRITE | EXECUTE | TRANSFER | ADMIN
```

- HandleTable 由 Native Task 持有；同一 Task 的 TCB 共享这张表。
- Handle 存放在 HandleTable 槽位；进程通过 `invoke` 调用对象方法。
- `duplicate` 需要 `ADMIN`，保持或收窄 rights；`close` 使旧 generation 失效。
- Handle 可随 IPC 消息传递；发送方需要 `TRANSFER` 权限。
- 每个对象有 `epoch`；owner 的 `revoke_all` 递增 epoch，使该对象所有旧 Handle 失效。
- 每 Task/服务器有 Handle 配额；`duplicate`/`transfer` 前检查。
- 权限模型采用线性 HandleTable；服务器重启时关闭旧 Handle。
- 对象命名由服务器 Endpoint 提供；内核 Handle 保持匿名。
- badge 用于 Endpoint 标识客户端；权限由 Handle 决定。

| right | 用途 |
| --- | --- |
| `READ` | 读取对象、接收消息、等待通知、查询状态 |
| `WRITE` | 修改对象、发送消息、映射可写 Frame |
| `EXECUTE` | 执行代码、映射可执行 Frame |
| `TRANSFER` | 通过 IPC 转移 Handle |
| `ADMIN` | 配置对象、duplicate、revoke_all |

- 失败返回 `BadHandle`、`Revoked`、`NoHandles`、`PermissionDenied` 或 `WrongType`。
- 对象引用归零时析构。

---

## 4. IPC

### 4.1 Endpoint

```rust
pub struct Message {
    pub label: u64,
    pub words: [u64; 8],
    pub handles: [Option<Handle>; 4],
}
```

- **Endpoint** 提供 `send`、`recv`、`call`、`reply`。`send` 为异步：消息入队后立即返回，队列有界（每个 Endpoint 最多缓存 64 条），满时返回 `QueueFull`，避免慢消费者导致内存无界增长；同步请求/应答使用 `call`/`reply`。
- 消息携带 label、最多 8 个字与 4 个 Handle；发送方需要 `TRANSFER` 权限。
- 大块数据通过共享 Frame 传递；消息只携带 Frame Handle。
- 阻塞、超时与取消由内核管理；等待队列按优先级排序。
- Endpoint 可绑定 badge；服务端区分客户端。

```mermaid
sequenceDiagram
    participant Client as Client
    participant EP as Endpoint
    participant Server as Server
    Client->>EP: call(label, words, handles)
    EP->>Server: recv()
    Server->>Server: 处理请求
    Server->>EP: reply(words, handles)
    EP-->>Client: 返回
```

### 4.2 Notification

```rust
pub struct Notification {
    bits: AtomicU64,
}

impl Notification {
    fn signal(&self, bits: u64);
    fn wait(&self, mask: u64, timeout: Option<Duration>) -> u64;
    fn poll(&self, mask: u64) -> u64;
    fn clear(&self, bits: u64);
}
```

- 异步事件：中断下半部、DMA 完成、定时器、进程退出。
- Notification 可绑定 TCB；`signal` 唤醒等待线程。
- 与 Endpoint 配合：服务器通过 Notification 等待设备事件。

### 4.3 共享 Frame

- Frame 是匿名内核对象 `{ phys, len }`，通过 Handle 访问。
- 大消息通过 Frame Handle 传递；接收方映射到自己的 VSpace，句柄权限决定只读/可写/可转移。
- 内核提供 `SYS_FRAME_ALLOC`、`SYS_FRAME_MAP`、`SYS_FRAME_UNMAP`、`SYS_FRAME_INFO`、`SYS_HANDLE_DUPLICATE` 与 `SYS_DMA_MAP_FRAME`。
- `SYS_FRAME_ALLOC(length, virt)` 从 DMA 池分配连续物理页、创建 Frame、映射到 `virt` 并返回帧句柄编号；`SYS_FRAME_INFO` 返回物理地址与长度，供驱动建立 DMA。
- 客户端保留原句柄，用 `SYS_HANDLE_DUPLICATE` 复制一份后通过 `SYS_SEND_HANDLE` 转移给服务；服务用 `SYS_FRAME_MAP` 映射同一物理页。
- 块协议：客户端通过 `BLOCK_ATTACH` 按槽位附加共享帧，消息字高 32 位为槽位、低 32 位为长度；每个请求携带 `{op, sector, length, tag}`，响应回显 `tag` 并按发送者与槽位定位对应帧。同一连接可以同时有多个未完成请求：服务端收下请求后交给设备异步队列，完成回收时再回复；驱动把帧物理地址登记给 virtio HAL，设备 DMA 直接读写帧。`BLOCK_DETACH` 按槽位或整体解除映射并关闭驱动侧句柄。
- VFS 协议：客户端发送 `VFS_ATTACH` 后，`OP_READ_FRAME`/`OP_WRITE_FRAME` 用 8 字节参数携带长度，文件内容直接读写客户端共享帧；服务端按发送者维护帧映射，重连时解除旧映射并关闭旧句柄，`VFS_DETACH` 清理帧与会话文件句柄。
- 块客户端登记 VFS 客户端帧；当后端把该帧作为缓冲区时，块客户端临时 attach 同一物理帧并让驱动 DMA 直达，实现 HolyFS 文档路径的零拷贝。HolyFS inode 缓存避免元数据读把数据帧切回自有帧。
- 块驱动在 attach 后用 `SYS_DMA_MAP_FRAME` 把帧物理区间映射进 DeviceGrant 的 IOMMU 域，并使用该物理地址作为 virtio DMA 缓冲；`SYS_DMA_SYNC` 接受本任务持有的 Frame 区间。
- Frame 的最后一个句柄释放时通过平台回调把物理区间归还 DMA 池；任务退出关闭句柄、客户端 detach 后回收。

---

## 5. 线程与 POSIX 映射

| POSIX 概念 | HK 实现 |
| --- | --- |
| 进程 | 共享 VSpace 的 TCB 集合 + POSIX Server 的进程表 |
| 线程 | 一个 TCB；libc 管理栈/TLS |
| `fork` | POSIX Server 创建新 VSpace、复制映射、创建 TCB |
| `clone` | POSIX Server 按标志共享 VSpace/fd/信号 |
| `exec` | 加载 ELF、创建新 VSpace、启动主 TCB |
| `wait` | 父进程 Endpoint 接收子进程退出事件 |
| 信号 | POSIX Server 维护 pending/blocked；libc 调用 handler |

内核只提供 TCB、VSpace、Endpoint、Notification、Frame 与 Handle；POSIX 语义在用户态完成。

### 5.1 原生线程

同地址空间线程已经落地，运行库提供以下系统调用：

- `SYS_THREAD_CREATE(entry, stack_top, argument, tls)` 在进程 VSpace 内创建 TCB 并返回线程编号；栈与 TLS 块由调用方从 Arena 分配。
- `SYS_THREAD_JOIN(thread)` 等待线程退出；等待期间当前线程由内核挂起，目标退出时唤醒。
- `SYS_THREAD_SET_TLS(tls)` 设置当前线程的 TLS 基址；每次调度切换时运行库写入线程指针寄存器（x86_64 为 `IA32_FS_BASE`）。
- `SYS_FUTEX_WAIT(address, expected)` 与 `SYS_FUTEX_WAKE(address, count)` 以共享内存字为键挂起与唤醒线程，地址按进程归属区分。
- 线程共享页表与 Arena；创建时把当前句柄对象授予新 TCB，之后新开的句柄只对所属线程可见。
- IPC 的接收者身份由进程主线程承担；线程专注计算、共享内存与 futex 同步。

### 5.2 协程

`hk-coroutine` 提供用户态有栈协程：每个协程持有独立栈与保存的执行现场，`Coroutine::resume` 从调用方栈切入协程栈，`suspend` 切回调用方。切换只保存 callee-saved 寄存器与栈指针，不经过内核。

- 栈尺寸由调用方指定并从服务堆分配；`Coroutine::new` 在栈顶布置入口与返回目标。
- 协程内可以恢复其他协程；恢复前的当前协程会被保存，返回后自动还原，嵌套恢复可用。
- 协程把等待点组织成顺序代码；真正的阻塞仍由 Endpoint、Notification 或 futex 承担。
- 同一线程同一时刻只运行一个协程；跨线程并发使用请让每个线程各自持有协程集合。

---

## 6. 同步

### 6.1 内核同步

- `SpinLock`：短临界区；禁用抢占。
- `IrqSpinLock`：自旋 + 关本地中断。
- `Mutex`：可睡眠；用于服务器内核态路径。
- `WaitQueue`：FIFO；IPC 阻塞与唤醒。
- `Once`、`Lazy`：初始化。

规则：

- `SpinLock` 持有期间使用固定分配与自旋等待。
- 中断上半部使用 `IrqSpinLock`。
- 锁顺序全局文档化；Hosted 调试构建启用 lockdep 式环检测。

### 6.2 用户态同步

- POSIX 互斥量、条件变量、信号量、futex 由 musl + `hk-abi` 与 POSIX Server 实现。
- futex 通过共享 Frame + Notification 实现等待/唤醒。
- 文件锁由 POSIX Server 管理。

---

## 7. 故障与退出

### 7.1 线程故障

| 故障 | 处理 |
| --- | --- |
| 页错误 | 发送到 Fault Endpoint；POSIX Server 处理 COW/映射/终止 |
| 非法指令 | 发送到 Fault Endpoint；POSIX Server 终止进程 |
| 除零/溢出 | 发送到 Fault Endpoint；默认终止进程 |
| 栈 guard | 发送到 Fault Endpoint；终止线程并回收栈 |
| IPC 超时 | 返回 `Timeout`；服务器决定重试 |
| 服务器崩溃 | Root Server 重启；句柄回收并重新分配 |

### 7.2 内核故障

- 内核保证失效、页表损坏、调度器死锁、中断风暴 → panic。
- panic：停世界、dump CPU 寄存器、栈回溯、当前 TCB、最近 trace。
- panic 路径使用静态缓冲与自旋锁，独立于堆与调度器。

### 7.3 退出

- TCB 退出后释放内核栈、TCB 对象与 Handle。
- POSIX 进程退出通过 Endpoint 通知父进程与 POSIX Server。
- Root Server 记录服务器退出状态并按策略重启。

---

## 8. 时间

```rust
pub struct Moment(u64);      // 单调 tick
pub struct Duration(u64);
pub struct CivilTime { ... }
```

- 每 CPU 一个时钟源；选择全局最稳定的作为单调时间。
- 定时器轮处理 IPC 超时、时间片、Notification 等待。
- RTC 在启动时读取，与当时的 `Moment` 组成民用时间锚点；之后民用时间由单调 tick 推进，亚秒按 tick 频率换算。
- 平台通过 `SYS_CIVIL_TIME` 返回 `CivilTime`；RTC 就绪前返回空值。
- 民用时间交给需要日历语义的服务；ext4 后端只在磁盘格式边界换算为 ext4 时间戳。
- Hosted 平台用虚拟时钟推进模拟民用时间。
- POSIX 时间通过 POSIX Server 与 libc 暴露。

---

## 9. 保证

1. 任意时刻，一个 TCB 最多在一个 CPU 上运行。
2. 每个 TCB 绑定一个 VSpace，并引用 Task 的 HandleTable。
3. Handle 只能由持有 `TRANSFER` 权限的线程传递。
4. IPC 消息中的 Handle 在接收方 HandleTable 中获得新槽位。
5. Endpoint 的发送/接收按优先级与 FIFO 规则调度。
6. 页错误通过 Fault Endpoint 交付；POSIX Server 处理语义。
7. POSIX PID 表、fd 表与信号状态由 POSIX Server 维护；内核提供 TCB 与 Handle。
8. 服务器崩溃后，Root Server 关闭旧句柄并重启服务。
9. 中断上半部使用固定分配与自旋锁。
10. panic 路径独立于调度器与堆。

---

## 10. 测试策略

| 层级 | 测试 |
| --- | --- |
| Hosted 单元 | run queue、优先级、Endpoint、Notification、Handle duplicate/close |
| Hosted 模拟 | 确定性调度、IPC 超时、句柄传递、服务器重启 |
| QEMU x86_64 | SMP 2/4 核、IPI、上下文切换、IPC 基准、故障注入 |
| QEMU AArch64/RISC-V | 同上，验证 PSCI/SBI、GIC/PLIC |
| POSIX 集成 | `fork`、`exec`、pthreads、信号、pipe、socket |
| fuzz | IPC 消息序列、Handle 操作序列、调度器随机种子 |
| 基准 | 上下文切换、IPC call/reply、Notification 唤醒、服务器重启时间 |

---

## 11. 相关文档

- [总体架构](architecture.md)
- [权限模型](permissions.md)
- [内存模型](memory.md)
- [POSIX 与 libc](posix.md)
- [服务器](servers.md)
- [平台与启动](platforms.md)
- [工程策略](engineering.md)
