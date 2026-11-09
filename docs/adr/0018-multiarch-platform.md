# ADR-0018：多架构平台分层与依赖契约

- 状态：**已接受**
- 日期：2026-11-09
- 关联：[平台与启动](../platforms.md)、[Crate 分层](../crates.md)、[ADR-0004：平台 HAL](0004-platform-hal.md)、[ADR-0016：HMP](0016-hmp-native-contract.md)

## 背景

x86_64、AArch64、RISC-V 与 Hosted 需要共享同一套微内核、HMP 与服务器语义。当前差异同时存在于固件解析、页表格式、引导协议、MMIO 寄存器访问和设备发现中；如果继续让平台 crate 各自实现这些通用机制，会出现以下问题：

- 同一个 ACPI/DTB 事实被多份代码解释，平台间行为不一致。
- 用户页表与 COW 依赖 x86 专用 PTE 位，无法移植到 AArch64/RISC-V。
- 内核或底层契约反向依赖平台实现，架构判断渗入微内核。
- 固件库、页表库与平台 crate 的依赖方向没有自动守卫，重构后容易回退。

本 ADR 固定多架构的分层、依赖方向与可移植 COW 模型。实现按阶段推进，但后续代码不得绕过本文定义的边界。

## 决策

### 1. 分层与依赖方向

```text
hk-base（共享契约）
  ├── hk-fw（固件描述：ACPI + DTB）
  └── hk-hal（平台 trait）
        └── hk-paging（通用用户页表 + COW）
              └── hk-platform-<arch>
                    └── platforms/<machine>
```

- `hk-base` 只定义地址、架构标识、`BootInfo`、时间与用户布局，保持零依赖。
- `hk-fw` 位于共享层，只依赖 `hk-base` 与固件库；不得依赖内核侧、平台侧或用户态 crate。
- `hk-hal` 只依赖 `hk-base`；不得依赖 `hk-fw`、`hk-paging` 或 `hk-platform-*`。
- `hk-paging` 只依赖底层共享契约与页表机制；不得依赖用户态 crate，也不得依赖具体平台 crate。
- `hk-platform-<arch>` 只做架构与硬件适配；微内核通过 `hk-hal`、`hk-fw`、`hk-paging` 的通用面工作，不出现 `cfg(target_arch)` 分支。
- `platforms/<machine>` 只负责入口、引导装配与机器特定初始化。

上述方向由 `cargo xtask deps` 强制检查。新增例外必须先在 ADR 中记录。

### 2. 固件描述的单一入口

- ACPI 与 DTB 的物理读取由调用方提供闭包，`hk-fw` 只负责校验、遍历和解释表结构。
- 外部固件库（`acpi`、`hermit-dtb`、`intel-iommu`、`amd-iommu`）只允许出现在 `hk-fw` 内；对外只暴露 HolyKernel 自己的类型。
- `FirmwareInfo` 是 ACPI/DTB 两种后端的统一结果；设备表只包含 MMIO 范围、IRQ、PCI ECAM、类别与 `compatible`，不含策略。
- 原始厂商表结构（DMAR/IVRS 等）属于实现细节；平台只消费中性的 `IommuInfo`/`IommuUnit`/`IommuReserved`。
- `hk-acpi` 的 RSDP/RSDT/XSDT 遍历已并入 `hk-fw`，不再保留独立机制 crate。

### 3. 可移植页表与 COW

- 用户地址空间统一使用 `hk-paging` 的 `UserSpace`；架构只选择页表后端（x86_64、AArch64、Sv39/Sv48）。
- 内核高半区映射在创建用户页表时从引导页表继承，不重建内核映射。
- COW 不使用架构专用 PTE 位，统一采用“帧引用计数 + 去写”模型：`fork` 递增可写页引用并清除写权限；写错误时按引用计数复制或恢复写权限。
- 页表帧、用户帧与 COW 引用的回收由通用分页层负责；平台只提供页表根、激活与 TLB 维护。
- checkpoint 依据帧引用计数识别共享页，恢复时只重建标准权限。

### 4. 引导与设备发现边界

- `BootInfo` 是唯一引导契约；Limine、DTB 与 UEFI 是后端，不把各自的原始结构传给微内核。
- 固件解析留在内核平台层，使用平台 scratch 窗口/HHDM；用户态 Device Manager 只通过只读系统调用取得规范化设备表。
- MMIO 寄存器访问统一使用 `hk-mmio` 的 `volatile-register` 包装；页表访问保持裸 volatile，不套寄存器类型。
- 设备发现只消费 `FirmwareDeviceTable`，不直接读物理内存，也不包含驱动策略。

### 5. 实施状态与迁移规则

本 ADR 是目标契约，按以下阶段收口；阶段未完成时，代码只能通过明确的过渡依赖工作：

- **P0（已完成）**：ADR、平台/crate/工程文档、三条依赖守卫；`hk-fw` 迁入共享层，`hk-acpi` 并入。
- **P1（已完成）**：`hk-fw` 产出统一的 `FirmwareInfo` 与 `FirmwareDeviceTable`；ACPI 表按表类型拆分；平台只消费中性 IOMMU 描述，DMAR/IVRS 原始字段不再从 `Tables` 暴露；ACPI/DTB 归一化测试已加入。
- **P2（核心已完成）**：`hk-paging` 提供 `UserSpace` 与 `FrameRefs`；x86 fork/缺页复制/释放改用帧引用计数，不再使用软件 COW PTE 位。checkpoint 用引用计数标记共享页并以 `PAGE_FLAG_SHARED` 跨层传递。页表编码、PCID、大页与 TLB 仍留在 x86 平台。
- **P3–P7**：x86 后端收口、AArch64/RISC-V、Limine/UEFI、设备发现与 conformance。

过渡依赖必须记录在对应阶段的提交说明中，并由 `cargo xtask deps` 保持可检查；不得把平台实现重新引入 `hk-base`、`hk-hal` 或 `hk-fw`。

## 后果

**收益**

- 固件、分页、引导与设备发现各有唯一通用层，平台差异集中到 `hk-platform-*`。
- 依赖方向可自动检查，减少“共享 crate 反向依赖内核/平台”的隐性耦合。
- COW 与用户页表不再绑定 x86 PTE 编码，为 AArch64/RISC-V 复用同一语义。
- 固件库与厂商类型被限制在包装层，用户态只接触规范化描述。

**成本与取舍**

- P2 仍需把 AArch64/RISC-V 页表后端接入 `hk-paging` 的 `UserSpace`；x86 的页表编码、PCID、大页与 TLB 继续保留在平台层。
- 统一 `FirmwareInfo` 会引入一次数据模型转换；对启动路径有少量分配与拷贝。
- 依赖守卫要求新增 crate 先确定侧别与允许依赖，增加提交前的结构成本。
- AArch64/RISC-V 的页表、TLB 与中断差异仍需后端实现，通用层不能消除所有架构代码。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 每个平台自行解析 ACPI/DTB | 实现直接；事实分裂，设备发现不可复用。 |
| 只支持 x86_64 | 范围最小；与“全平台全架构”目标冲突。 |
| 继续在平台 crate 保留用户页表与 COW | 迁移成本低；AArch64/RISC-V 无法共享语义，COW 绑定 x86 PTE。 |
| 引入完整的外部页表/固件框架并直接暴露其类型 | 复用成熟实现；ABI 与依赖面扩大，厂商/架构类型泄漏到上层。 |
| 仅在 CI 做架构编译、不做依赖守卫 | 成本低；重构后容易回退，问题在启动期才暴露。 |

## 架构一致性检查

- 微内核仍只通过 `hk-hal`、`hk-fw`、`hk-paging` 的通用面访问平台；Task/TCB、Arena、HandleTable、Endpoint、Notification、HMP 的语义不随架构变化。
- 固件描述与设备表只提供机制；驱动策略、DeviceGrant 与设备管理仍由用户态服务决定。
- COW 的帧引用计数与 HMP `Frame`/`Region` 记账一致；checkpoint 不依赖平台 PTE 位。
- `hk-fw` 与 `hk-paging` 都不持有策略对象，不绕过 Handle/Rights 授予规则。
- 依赖守卫保持共享契约、内核侧与用户态的边界，服务器重启与最小权限模型不变。

## 相关

- [ADR-0004：平台 HAL](0004-platform-hal.md)
- [ADR-0016：Holy Memory Plane](0016-hmp-native-contract.md)
- [平台与启动](../platforms.md)
- [Crate 分层](../crates.md)
- [工程策略](../engineering.md)
