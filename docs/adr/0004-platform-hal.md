# ADR-0004：平台 HAL 与 Hosted 模拟器

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[平台与启动](../platforms.md)、[总体架构](../architecture.md)

## 背景

微内核需要跨 x86_64、AArch64、RISC-V 与 Hosted 共享同一套对象与 IPC；硬件差异需要隔离。裸机调试与 CI 需要确定性模拟器。

## 决策

1. 内核只依赖 `hk-hal` trait；架构判断位于 `hk-hal` 实现或 platform crate。
2. HAL 定义：`Platform`、`Mmu`、`InterruptController`、`Timer`、`Console`、`DmaOps`、`PciConfig`、`Entropy`、`Power`。
3. 每个架构一个 `hk-platform-*`；每个机器一个 `platforms/*` 目录。
4. 统一 `BootInfo`：内存 map、内核镜像、帧缓冲、direct map、DTB/RSDP、服务器模块。
5. Hosted 模拟器是一等平台：虚拟时钟、虚拟 CPU、模拟设备、故障注入。
6. Hosted 内部使用 `std`；HAL 与内核接口保持 `no_std`。
7. 每个平台通过 conformance suite 后标记支持。

## 后果

**收益**

- 内核与服务器语义跨架构一致。
- Hosted 支持 CI、TDD、IPC 回放、服务器重启测试。
- 多架构移植有明确清单。

**成本与取舍**

- HAL trait 调用有间接开销；通过单态化/内联缓解。
- 抽象边界：缓存维护、页表格式、IPI 需要明确。
- 维护多个目标增加 CI 成本。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 只支持 x86_64 | 范围小；多架构目标推迟。 |
| 核心内 `cfg(target_arch)` | 实现直接；架构逻辑分散。 |
| 微内核式平台服务 | 隔离强；IPC 成本高。 |

## 架构一致性检查

- HAL 使用微内核类型与语义。
- BootInfo 包含服务器模块，Root Server 从模块加载 ELF。
- 平台层保持硬件相关的最小实现。

## 相关

- [ADR-0001：Rust no_std](0001-rust-no-std.md)
- [ADR-0002：微内核](0002-microkernel.md)
