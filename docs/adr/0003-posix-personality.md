# ADR-0003：用户态 POSIX Personality

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[POSIX 与 libc](../posix.md)、[服务器](../servers.md)、[VFS](../vfs.md)

## 背景

应用需要 POSIX.1 接口；内核保持最小。POSIX 进程、fd、信号、线程等状态适合放在用户态服务器中维护，并通过 IPC 与 VFS/Network/内核协作。

## 决策

1. POSIX.1 语义由用户态 **POSIX Server** 实现。
2. musl 提供 C 运行时与 POSIX 头文件；`hk-abi`/`hk-crt` 提供 shim、启动与 TLS；应用重新编译。
3. POSIX Server 维护 PID、进程组、会话、fd 表、信号状态、cwd/root、凭证。
4. 文件操作通过 **VFS Server**；socket 通过 **Network Server**。
5. `fork` 通过 Arena 复制与 COW；`exec` 通过 ELF 加载器；`wait` 通过 Endpoint。
6. pthreads 映射到 TCB；libc 管理栈、TLS、join、取消。
7. 信号由 POSIX Server 管理 pending/blocked，libc trampoline 调用 handler。
8. 1.0 聚焦源码兼容的 POSIX.1；Linux 二进制 ABI 属于后续版本。

## 后果

**收益**

- POSIX 语义位于用户态兼容层；内核提供机制，兼容层可独立演进。
- 应用生态通过 musl + `hk-abi` 接入；C/C++/Rust 共用 POSIX ABI。
- POSIX Server 可重启；应用按策略恢复或终止。

**成本与取舍**

- POSIX Server 是较大可信组件；需要严格测试与审计。
- 每次 POSIX 调用经过 libc → POSIX Server → VFS/kernel IPC；需要快速路径。
- `fork`/信号/线程语义复杂；需要 POSIX 测试套件。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 内核内 POSIX | 性能直接；内核膨胀，可信基扩大。 |
| Linux ABI 兼容层 | 应用范围大；syscall 面广，1.0 风险高。 |
| 源码级 POSIX.1 | 接口清晰；应用需要重新编译。 |

## 架构一致性检查

- POSIX Server 是用户态服务，通过 Endpoint 与 VFS/Network/内核协作。
- POSIX PID、fd、信号与 cwd 由 POSIX Server 维护；内核提供机制。
- musl 是兼容环境的主 C libc；`hk-abi` 是唯一 syscall shim 边界。

## 相关

- [ADR-0007：musl 与 hk-libc](0007-posix-libc.md)
- [ADR-0006：VFS](0006-vfs.md)
