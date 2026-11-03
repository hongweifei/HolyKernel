# HolyKernel 文档索引

本目录是 HolyKernel 的设计与工程文档。设计阶段以文档为规范；代码落地后，文档与代码同步演进。

## 阅读顺序

1. [章程](charter.md) — 微内核模型、Native Task、句柄、POSIX 兼容层。
2. [总体架构](architecture.md) — 内核对象、IPC、服务器、启动、安全模型。
3. [执行模型](execution.md) — TCB、调度、Endpoint、Notification、Handle。
4. [权限模型](permissions.md) — HandleTable、rights、epoch 撤销、配额、审计。
5. [内存模型](memory.md) — Untyped、Frame、VSpace、COW、DMA。
6. [服务器](servers.md) — Root Server、设备管理、驱动、VFS、POSIX、网络、重启。
7. [网络](network.md) — Network Server、TCP/IP、DHCP、DNS、POSIX socket。
8. [HolyShell 与 Holy 命令](shell.md) — 原生命令、结构化管道、POSIX 兼容入口。
9. [VFS](vfs.md) — 挂载表、路径解析、文件句柄、页缓存。
10. [HolyFS 文件系统](holyfs.md) — 可作主根、POSIX + 整文档、COW、checksum、快照。
11. [POSIX 与 libc](posix.md) — POSIX 兼容环境、syscall shim、进程/线程/信号、ELF。
12. [HolyDoc 与 UI](ui.md) — 可选原生显示服务与真彩界面。
13. [Holy 原生体验](experience.md) — Workbench、Toolchain、Checkpoint、符号化调试。
14. [平台与启动](platforms.md) — HAL、BootInfo、多架构启动、用户态驱动。
15. [工程策略](engineering.md) — 构建、CI、测试、可观测性、`unsafe`。
16. [路线图](roadmap.md) — M0–M9 与退出标准。
17. [术语表](glossary.md) — 微内核与 POSIX 术语。

## 文档地图

```text
docs/
├── README.md            本文件
├── charter.md           章程：微内核模型、Native Task、句柄、POSIX 兼容层
├── architecture.md      总体架构
├── execution.md         TCB、调度、IPC、Handle
├── permissions.md       HandleTable、rights、epoch、配额、审计
├── memory.md            Untyped、Frame、VSpace、COW、DMA
├── servers.md           Root Server、设备管理、驱动、VFS、POSIX
├── network.md           Network Server、TCP/IP、DHCP、DNS、socket
├── shell.md             HolyShell、Holy 命令、结构化管道、POSIX bridge
├── vfs.md               VFS 与文件系统后端
├── holyfs.md            可作主根、POSIX + 整文档、COW、checksum、快照
├── posix.md             POSIX 兼容环境、musl 与 hk-abi
├── ui.md                HolyDoc 与真彩 Display Server
├── experience.md        Workbench、Toolchain、Checkpoint、符号化调试
├── platforms.md         HAL、BootInfo、多架构启动
├── engineering.md       构建、CI、测试、可观测性、安全策略
├── roadmap.md           里程碑、退出标准、风险
├── glossary.md          微内核与 POSIX 术语
└── adr/                 架构决策记录
    ├── README.md        ADR 索引与模板
    ├── 0001-rust-no-std.md
    ├── 0002-microkernel.md
    ├── 0003-posix-personality.md
    ├── 0004-platform-hal.md
    ├── 0005-handles.md
    ├── 0006-vfs.md
    ├── 0007-posix-libc.md
    ├── 0008-preemptive-smp.md
    ├── 0009-network.md
    └── 0010-holyfs.md
```

## 状态标记

- **已接受**：已通过 ADR，代码遵循。
- **设计中**：方向明确，细节可能调整，变更更新文档。
- **待定**：待决策，以 `TBD` 标注，并链接到待写 ADR。
- **架构边界**：章程明确的微内核构件集合。

当前所有子系统均为**设计中**。`v0.1.0-design` 的目标是让内核机制、服务器划分、POSIX 兼容层与测试策略都有可执行的接口草图与退出标准。

## 约定

- 需求强度使用 **必须 / 应该 / 可以**，例外通过 ADR 记录。
- 代码片段是接口草图；最终实现以代码为准，内核代码使用 `no_std` 语义。
- 内核文档使用 TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Handle 等术语。
- POSIX 服务器、VFS、文件系统与驱动属于用户态；内核文档描述机制与边界。
- 文档与代码冲突时，以 [章程](charter.md) 与已接受的 ADR 为准。

## 贡献前检查

1. 变更与 [章程](charter.md) 的微内核模型对齐。
2. 内核代码只包含机制；策略位于服务器。
3. 新增平台差异位于 `platforms/` 或 `crates/hk-platform-*`。
4. 新增依赖通过 [工程策略](engineering.md) 的依赖审查。
5. 接口有测试策略，覆盖失败路径、IPC 断开与服务器重启。
