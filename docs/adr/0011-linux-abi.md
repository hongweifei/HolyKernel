# ADR-0011：Linux 二进制 ABI 兼容层

- 状态：**已接受**
- 日期：2026-11-04
- 关联：[应用兼容](../compat.md)、[POSIX 与 libc](../posix.md)、[路线图](../roadmap.md)

## 背景

当前 1.0 目标聚焦源码兼容：应用针对 musl + `hk-abi` 重新编译。为了让更多软件低成本迁移，需要同时覆盖未修改的 Linux ELF 二进制。把 Linux syscall 语义放进内核会扩大可信基，并破坏微内核边界；因此兼容层应位于用户态，内核只提供通用的 ABI 转发机制。

## 决策

1. **三层兼容**：L1 源码兼容（musl + `hk-abi`）、L2 Linux ELF 二进制兼容（Linux ABI Server）、L3 VM/容器兜底。
2. **内核只做机制**：每任务维护 `Personality { Native, Posix, Linux }`；Native 使用现有 `int 0x80`，Linux 使用 `syscall`；内核保存上下文并把原始 syscall 转发到已注册的 ABI Server Endpoint，不解析 Linux 语义。
3. **Linux ABI Server 运行在用户态**：负责 syscall 分派、ELF/`PT_INTERP`、auxv、TLS、信号帧、`clone`/futex、`mmap`、`epoll`/`ioctl`、`/proc`/`/sys` 最小视图与 vDSO。
4. **复用现有后端**：POSIX Server、VFS、Network、Display、Device 提供进程、文件、socket、终端与设备能力；兼容层不复制这些状态。
5. **x86_64 优先**：先支持静态 musl/glibc，再支持动态 glibc；AArch64/RISC-V 在后续里程碑增量接入。
6. **源码兼容工具链**：提供 `hk-abi` 头文件、musl sysroot、CMake/Meson/Autotools toolchain 与 `holybuild`。
7. **支持自带 libc**：允许应用静态链接自有 libc，但必须实现 `hk-abi` 后端；musl 为一等支持。
8. **安全边界**：Linux ABI Server 是用户态高权限组件，可重启；每个兼容应用独立 VSpace/Native Task，权限由 Handle 管理；`/proc`/`/sys` 只暴露最小视图。
9. **明确非目标**：不承诺内核模块、完整容器编排、GPU/专有驱动与完整 `io_uring`/eBPF 原生兼容；这些走 L3 兜底。

## 后果

**收益**

- 内核保持最小；Linux 兼容层可独立演进、重启与测试。
- L1 覆盖大多数 POSIX 源码应用；L2 覆盖常见 Linux 二进制；L3 处理系统级软件。
- 复用 POSIX Server、VFS、Network 与 Handle 权限模型，避免两套状态。
- 兼容性边界清晰，可按真实应用逐步扩大 syscall 覆盖。

**成本与取舍**

- Linux syscall 面广，glibc 的 TLS、信号、`/proc`、auxv 与动态加载器需要大量适配。
- 每次 foreign syscall 需要一次 IPC；需要 vDSO、缓存、批量与热路径优化。
- Linux ABI Server 是高权限组件，需要沙箱、审计与 fuzz。
- 二进制兼容会延长 1.0 前的开发周期；通过 P1/P2/P3 分批与真实应用门禁控制范围。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 内核内 Linux syscall | 性能直接；内核膨胀、可信基扩大、跨架构复杂。 |
| 仅 L1 源码兼容 | 实现简单；大量未修改二进制无法运行。 |
| 仅 VM/容器 | 兼容性最好；资源开销大，无法作为原生路径。 |
| 用户态 Linux ABI + 分层兜底 | 内核最小；实现量大，但边界清晰、可增量。 |

## 架构一致性检查

- 内核只增加 personality 与 foreign syscall 转发，不引入 Linux 对象或策略。
- Linux ABI Server 通过 Endpoint 与 POSIX Server/VFS/Network 协作；进程与 fd 状态仍由 POSIX Server 维护。
- ELF、Frame 映射、信号与线程遵循现有 VSpace/TCB/Handle/Notification 机制。
- L2 应用与 L1 应用共享 VFS、Network、Display 与权限模型，不建立平行内核接口。
- 兼容层崩溃由 Root Server 重启；应用按策略恢复或终止。

## 相关

- [ADR-0003：用户态 POSIX Personality](0003-posix-personality.md)
- [ADR-0007：musl 移植 + 薄 hk-libc](0007-posix-libc.md)
- [ADR-0009：用户态网络服务](0009-network.md)
- [应用兼容与 Linux ABI](../compat.md)
