# 架构决策记录（ADR）

本目录记录 HolyKernel 的架构决策。实现遵循已接受的 ADR；决策演进通过新的 ADR 完成。

## 索引

| 编号 | 决策 | 状态 |
| --- | --- | --- |
| [0001](0001-rust-no-std.md) | Rust `no_std` 内核 + Rust 用户态服务器 | 已接受 |
| [0002](0002-microkernel.md) | 句柄型微内核 | 已接受 |
| [0003](0003-posix-personality.md) | 用户态 POSIX Personality | 已接受 |
| [0004](0004-platform-hal.md) | 平台 HAL 与 Hosted 模拟器 | 已接受 |
| [0005](0005-handles.md) | Handle 与 IPC | 已接受 |
| [0006](0006-vfs.md) | 用户态 VFS 与 Path Server | 已接受 |
| [0007](0007-posix-libc.md) | musl 移植 + 薄 hk-libc | 已接受 |
| [0008](0008-preemptive-smp.md) | 抢占式对称多处理 | 已接受 |
| [0009](0009-network.md) | 用户态网络服务 | 已接受 |
| [0010](0010-holyfs.md) | HolyFS 文件系统（Holy 对象模型） | 已接受 |
| [0011](0011-linux-abi.md) | Linux 二进制 ABI 兼容层 | 已接受 |
| [0012](0012-musl-dynamic-abi.md) | musl 动态 ABI 兼容 | 已接受 |
| [0013](0013-net-stack.md) | virtio-net 与 smoltcp 网络栈 | 已接受 |
| [0014](0014-native-image.md) | 原生任务镜像 | 已接受 |
| [0015](0015-compatibility-server.md) | Compatibility Server 与多来源 UI | 已接受 |
| [0016](0016-hmp-native-contract.md) | Holy Memory Plane 作为唯一内存面 | 已接受 |
| [0017](0017-holy-cadence.md) | Holy Cadence Model 作为原生调度契约 | 已接受 |
| [0018](0018-multiarch-platform.md) | 多架构平台分层、依赖契约与可移植 COW | 已接受 |

## 模板

```markdown
# ADR-XXXX：标题

- 状态：提议 / 已接受 / 已取代 / 已归档
- 日期：YYYY-MM-DD
- 关联：

## 背景

为什么需要这个决策？有哪些约束？

## 决策

我们决定做什么？用可执行的规则描述。

## 后果

收益 / 成本与取舍 / 中性。

## 备选方案

每个方案的取舍。

## 架构一致性检查

这个决策如何与微内核模型协作：Task/TCB、Cadence/Meter、Beat、Arena、HandleTable、Endpoint、Notification、HMP（Capacity/Frame/Region）、服务器？
```

## 规则

1. 触及内核对象、Handle、IPC、Arena、POSIX 兼容层、服务器边界或 ABI 的变更先写 ADR。
2. ADR 包含“架构一致性检查”。
3. 已接受的 ADR 通过新 ADR 演进；新 ADR 取代时保留原文并链接新 ADR。
4. 代码与 ADR 冲突时，以章程与已接受 ADR 为准。
5. ADR 与实现同 PR 更新。

## 相关

- [章程](../charter.md)
- [总体架构](../architecture.md)
- [路线图](../roadmap.md)
- [文档索引](../README.md)
