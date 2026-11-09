# HolyKernel 文档索引

本目录是 HolyKernel 的设计与工程文档。设计阶段以文档为规范；代码落地后，文档与代码同步演进。

## 阅读顺序

1. [章程](charter.md) — 微内核模型、Native Task、句柄、POSIX 兼容层。
2. [总体架构](architecture.md) — 内核对象、IPC、服务器、启动、安全模型。
3. [执行模型](execution.md) — Task/TCB、Endpoint、Notification、Handle。
4. [Holy Cadence Model](scheduling.md) — 原生调度契约、Cadence/Meter、Tempo、Beat、Lending。
5. [权限模型](permissions.md) — HandleTable、rights、epoch 撤销、配额、审计。
6. [内存模型（HMP）](hmp.md) — 唯一内存面、Capacity/Frame/Region、统一能力面与授予面、Provider、Ledger。
7. [服务器](servers.md) — Root/Tempo、设备管理、驱动、VFS、POSIX、网络、重启。
8. [网络](network.md) — Network Server、TCP/IP、DHCP、DNS、POSIX socket。
9. [HolyShell 与 Holy 命令](shell.md) — 原生命令、结构化管道、POSIX 兼容入口。
10. [VFS 与 Path Server](vfs.md) — Holy 原生命名空间、盘符投影、路径与页缓存。
11. [HolyFS 文件系统](holyfs.md) — Holy 对象、修订、快照、标签、COW、checksum。
12. [POSIX 与 libc](posix.md) — POSIX 兼容环境、syscall shim、进程/线程/信号、ELF。
13. [应用兼容、Linux ABI 与 Compatibility Server](compat.md) — POSIX/Linux/Harmony personality、glibc/musl 动态库、ABI 兼容层与移植工具链。
14. [HolyDoc 与 UI](ui.md) — 可选原生显示服务与真彩界面。
15. [Holy 原生体验](experience.md) — Workbench、Toolchain、Checkpoint、符号化调试。
16. [平台与启动](platforms.md) — HAL、BootInfo、多架构启动、用户态驱动。
17. [Crate 分层](crates.md) — 工作区 crate 清单、依赖方向与新增服务清单。
18. [工程策略](engineering.md) — 构建、CI、测试、可观测性、`unsafe`。
19. [路线图](roadmap.md) — M0–M11 与退出标准。
20. [术语表](glossary.md) — 微内核、HMP、HCM 与兼容层术语。

## 文档地图

```text
docs/
├── README.md            本文件
├── charter.md           章程：微内核模型、Native Task、句柄、POSIX 兼容层
├── architecture.md      总体架构
├── execution.md         Task/TCB、Endpoint、IPC、Handle
├── scheduling.md        Holy Cadence Model：Cadence/Meter、Tempo、Beat、Lending
├── permissions.md       HandleTable、rights、epoch、配额、审计
├── hmp.md               Holy Memory Plane：唯一内存面、Capacity/Frame/Region、授予、Provider、Ledger
├── servers.md           Root Server、设备管理、驱动、VFS、POSIX
├── network.md           Network Server、TCP/IP、DHCP、DNS、socket
├── shell.md             HolyShell、Holy 命令、结构化管道、POSIX bridge
├── vfs.md               VFS 与 Path Server：盘符、路径、页缓存
├── holyfs.md            Holy 对象、修订、快照、标签、COW、checksum
├── posix.md             POSIX 兼容环境、musl 与 hk-abi
├── compat.md            Compatibility Server、Linux ABI、glibc/musl、移植工具链
├── ui.md                HolyDoc 与真彩 Display Server
├── experience.md        Workbench、Toolchain、Checkpoint、符号化调试
├── platforms.md         HAL、BootInfo、多架构启动
├── crates.md            工作区 crate 分层、依赖规则、服务清单
├── engineering.md       构建、CI、测试、可观测性、安全策略
├── roadmap.md           里程碑、退出标准、风险
├── glossary.md          微内核、HMP 与兼容层术语
├── adr/                 架构决策记录
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
    ├── 0010-holyfs.md
    ├── 0011-linux-abi.md
    ├── 0012-musl-dynamic-abi.md
    ├── 0013-net-stack.md
    ├── 0014-native-image.md
    ├── 0015-compatibility-server.md
    ├── 0016-hmp-native-contract.md
    ├── 0017-holy-cadence.md
    └── 0018-multiarch-platform.md
└── holy/                 HolyDoc 启动文档与内置帮助（*.hd，非设计文档）
```

## 状态标记

- **已接受**：已通过 ADR，代码遵循。
- **已实现 / 现行**：代码与文档反映当前实现。
- **设计中**：方向明确，细节随设计演进更新。
- **待定**：待决策，以 `TBD` 标注，并链接到待写 ADR。
- **架构边界**：章程明确的微内核构件集合。

HMP、HCM 与多架构平台分层已通过 ADR-0016、ADR-0017 与 ADR-0018 接受；VFS 与 HolyFS 文档标记为现行；其余架构文档为设计中，代码随里程碑落地。`v0.1.0` 已交付 M0–M5，当前焦点是 M6 底层收敛（HMP 对象面、Capacity 帧库、Region 物化、COW、MMIO/DMA、HMP ABI 与 Holy Cadence 调度）。

## 约定

- 需求强度使用 **必须 / 应该 / 可以**，例外通过 ADR 记录。
- 代码片段是接口草图；最终实现以代码为准，内核代码使用 `no_std` 语义。
- 内核文档使用 Task、Cadence、Meter、Tempo、Beat、Grant、Lending、TCB、Arena、HandleTable、Endpoint、Notification、HMP（Capacity/Frame/Region）、Handle 等术语。
- POSIX Server、VFS/Path、文件系统与驱动属于用户态；内核文档描述机制与边界。
- 文档与代码冲突时，以 [章程](charter.md) 与已接受的 ADR 为准。

## 贡献前检查

1. 变更与 [章程](charter.md) 的微内核模型对齐。
2. 内核代码只包含机制；策略位于服务器。
3. 内存相关变更使用 [HMP](hmp.md) 的对象面与 [ADR-0016](adr/0016-hmp-native-contract.md) 的授予规则；HMP 是唯一内存契约。
4. 调度相关变更使用 [Holy Cadence Model](scheduling.md) 与 [ADR-0017](adr/0017-holy-cadence.md) 的 Cadence/Meter 契约。
5. 新增平台差异位于 `platforms/` 或 `crates/kernel/hk-platform-*`。
6. 新增依赖通过 [工程策略](engineering.md) 的依赖审查。
7. 接口有测试策略，覆盖失败路径、IPC 断开与服务器重启。
