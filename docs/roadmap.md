# 路线图

状态：**设计中** · 上级：[总体架构](architecture.md)

本文定义 M0–M11 里程碑、退出标准、决策门与风险。路线按底层优先推进：先固定 HMP 对象面与内存机制，再完成平台、设备、存储、UI 与兼容层。

---

## 1. 排序原则

1. **底层优先**：HMP 对象面 → 内存机制 → 分页与 MMIO → 平台与引导 → 设备与存储 → UI → 兼容。
2. **先 Hosted，后裸机**：内核与 IPC 在确定性模拟器中先跑通。
3. **先内核机制，后服务器**：TCB、Arena、HandleTable、Endpoint、Notification、HMP 优先。
4. **单对象面**：Arena、Capacity、Frame、Region、Capability、Provider、Ledger 覆盖内核与用户态。
5. **纵向切片**：每个里程碑形成可演示、可冒烟、可回归的完整链路。
6. **平台与引导先行**：UI 与兼容层建立在稳定的 HAL、BootInfo、HMP 与设备链路之上。

---

## 2. 里程碑总览

| 里程碑 | 主题 | 关键产物 | 决策门 |
| --- | --- | --- | --- |
| M0 | 章程、架构、脚手架 | 文档、workspace、CI、xtask、Hosted stub | G0：架构批准 |
| M1 | Hosted 微内核 | TCB、Arena、HandleTable、Endpoint、Notification、HMP | G1：内核对象冻结 |
| M2 | x86_64 启动 | Limine、BootInfo、Root Server、IPC | — |
| M3 | 句柄与内存 | Handle duplicate/close、HMP 对象、COW、SMP、调度对象面预留 | G2：句柄模型冻结 |
| M4 | 用户态驱动与 VFS | DeviceGrant、virtio、VFS、ext2/tmpfs/devfs/procfs | G3：VFS 接口冻结 |
| M5 | 网络 | virtio-net、Network Server、IPv4/IPv6、TCP/UDP、DHCP/DNS | — |
| M6 | 底层收敛 | HMP 对象面、Capacity 帧库、Region 物化、COW、MMIO/DMA、HMP ABI、Holy Cadence 调度 | G4：内存对象面冻结 |
| M7 | 平台与引导 | HAL、BootInfo、ACPI/DTB、x86_64、AArch64、Limine/UEFI | G5：平台抽象冻结 |
| M8 | 设备与存储 | Device Manager、virtio 驱动、DMA/IOMMU、VFS、HolyFS | G6：设备与存储接口冻结 |
| M9 | 原生 UI 与工具链 | Display、指针、Console、Input、HolyDoc、Workbench、Toolchain Server | — |
| M10 | 兼容 | Compatibility Server、POSIX、Linux ABI、Harmony | G7：兼容 ABI 冻结 |
| M11 | 1.0 与多架构 | AArch64/RISC-V、conformance、硬化、发布 | G8：1.0 |

```mermaid
flowchart LR
    M0["M0 章程与脚手架"] --> M1["M1 Hosted 微内核"]
    M1 --> M2["M2 x86_64 启动"]
    M2 --> M3["M3 句柄与内存"]
    M3 --> M4["M4 驱动与 VFS"]
    M4 --> M5["M5 网络"]
    M5 --> M6["M6 底层收敛"]
    M6 --> M7["M7 平台与引导"]
    M7 --> M8["M8 设备与存储"]
    M8 --> M9["M9 原生 UI 与工具链"]
    M9 --> M10["M10 兼容"]
    M10 --> M11["M11 1.0 与多架构"]
```

---

## 3. 已完成基础（M0–M5）

- **M0**：章程、架构、HMP、执行、服务器、VFS、UI、平台、工程、路线图、术语与 ADR 文档；workspace、`xtask`、CI、Hosted stub。
- **M1**：Hosted 微内核；TCB、Arena、HandleTable、Endpoint、Notification、HMP 对象、确定性仿真。
- **M2**：x86_64 启动；Limine、BootInfo、串口、GDT/IDT、分页、LAPIC/IOAPIC、帧缓冲控制台、用户态服务与 IPC。
- **M3**：句柄与内存；Handle duplicate/close/transfer、COW、SMP、调度对象面预留、用户态线程、审计与 trace。
- **M4**：用户态驱动与 VFS；DeviceGrant、virtio-blk/input、Block Server、ext4/HolyFS、VFS 与路径服务、共享 Frame 数据路径。
- **M5**：网络；virtio-net 驱动、Network Server、IPv4/IPv6、TCP/UDP、DHCP、DNS、Socket Handle。

---

## 4. M6 — 底层收敛

### 交付

- **HMP 共享契约**：`Arena`、`Capacity`、`Frame`、`Region`、`Capability`、`Provider`、`Ledger`、`Mappable`、`Handle`、`Rights`；内核与用户态共享同一对象定义。
- **Arena**：任务内存根；创建、Region 映射、增长、检查点、恢复与销毁；栈、堆与工作区归入 Arena。
- **Capacity**：从完整 BootInfo 内存描述建立 System/User/Dma/Gpu/Numa 容量；拆分、配额、Frame 分配与归还；覆盖全部可用物理内存。
- **Frame**：FrameId、所属 Capacity、共享引用、pin/unpin、export/import；最后一个引用归还 Capacity，归还前清零。
- **Region**：Arena 内唯一映射视图；map/unmap/protect/fault/grow；COW、GUARD、PINNED、SEALED、DONT_FORK、GROW 标志。
- **COW**：`RegionFlags::COW` 为唯一契约状态；匿名 COW 走内核快速路径，绑定 Provider 的能力交给 Provider。
- **页表与 MMIO**：Region 物化为页表；MMIO、DMA、帧缓冲与自动窗口统一走 Region；页表机制留在实现层。
- **HMP ABI**：Arena/Capacity/Frame/Region/Capability/Provider/Ledger 操作统一；Memory Server、Device Manager、驱动、VFS、Display 与客户端接入同一对象面。
- **Provider**：Anonymous、Revision、Surface、TaskImage、DeviceAllocation 的 fault、writeback、evict、commit、scanout、pin、export、swap 契约。
- **Ledger**：Arena、Capacity、Frame、Region 与能力的容量、提交/常驻、峰值、软/硬上限与压力记账；Handle 转移时记账随对象转移。
- **Holy Cadence**：Cadence/Meter 对象、Tempo Server、Beat、Task 级与 TCB 级两阶段调度、Lending、Tickless、idle TCB、亲和性与负载均衡。
- **调度 ABI**：`SYS_CADENCE_*`、`SYS_METER_INFO`、`SYS_TASK_ATTACH_CADENCE`；Root 服务清单授予初始 Cadence。

### 退出标准

1. HMP 宿主单测覆盖 Arena、Capacity 拆分/合并、Frame 引用、Region 权限/COW、Ledger、Provider fault。
2. x86_64 的 `boot`、`display`、`pointer`、`fork`、`checkpoint`、`net`、`holyfs` 冒烟全绿。
3. 所有用户地址映射对应 Region；所有物理分配来自 Capacity/Frame。
4. 服务、驱动、客户端与 demo 使用同一组 HMP 对象与 Rights。
5. AArch64 目标编译通过；`memory` conformance 覆盖对象面统一语义。
6. `cargo xtask lint`、`cargo test --workspace`、`cargo xtask docs` 全绿。
7. Cadence/Meter、Tempo 与 Beat 的机制与冒烟全绿；空闲 CPU 用户态占用 0%，同权重份额误差 ≤2%，唤醒延迟 ≤100µs。Realtime 的硬保证与准入策略在 M11 验收。

---

## 5. M7 — 平台与引导

### 交付

- HAL：`Platform`、`Mmu`、`InterruptController`、`Timer`、`Console`、`DmaOps`、`PciConfig`、`Entropy`、`Power`、`Iommu`、`CacheOps`。Hosted 与 x86_64 均已实现 `Platform`；x86_64 另补 CPUID 特性矩阵、RDRAND 熵源、XSAVE/AVX 上下文、SMEP/SMAP、用户镜像 W^X 与 guard page；运行时硬件访问已收敛到 `Platform`。
- BootInfo：内存 map、内核镜像、帧缓冲、direct map、DTB/RSDP、服务模块；统一校验。
- `hk-fw`：ACPI 与 DTB 解析，统一产出 `FirmwareInfo` 与 `FirmwareDeviceTable`（MMIO、IRQ、PCI ECAM、兼容字符串）。已落地 ACPI MADT（x86 IOAPIC/本地 APIC/中断覆盖，接入 I/O APIC 初始化）、DTB 内存/CPU/通用设备描述与中性 IOMMU 描述；Arena/Capacity 物化输入待平台后端接入。
- `hk-mmio`：包装 `volatile-register`，统一 LAPIC、I/O APIC、PCI ECAM 与 MSI-X 的 MMIO 读写；内核与用户态驱动共用。
- `page_table_entry`：统一 x86_64 PTE 位与属性编码（`PTF`/`MappingFlags`）；页表遍历、PCID、大页与 TLB 由平台维护，COW 共享状态由 `hk-paging::FrameRefs` 维护。
- x86_64 平台：GDT/TSS/IDT、LAPIC/IOAPIC、PCI legacy/ECAM、串口、帧缓冲、SMP。
- AArch64 平台：PL011、GICv3、Generic Timer、PSCI、DTB、页表。
- 引导：`hk-boot` 已提供 `BootBackend` 抽象、Limine（x86_64/AArch64/RISC-V）与 DTB（AArch64/RISC-V `-kernel`）后端；UEFI loader 待后续；`platforms/*` 启动目录与镜像。
- `xtask`：aarch64 构建、运行与冒烟模式。

### 退出标准

1. Hosted、x86_64、AArch64 三目标构建成功。
2. x86_64 与 AArch64 启动到内核 Shell；串口输出 CPU、HMP、设备与启动耗时。
3. ACPI 与 DTB 后端产出等价的 Arena/Capacity/设备描述。
4. `boot`、`irq`、`memory` conformance 在两个架构通过。
5. `cargo xtask lint`、`cargo test --workspace`、全部冒烟通过。

---

## 6. M8 — 设备与存储

### 交付

- Device Manager：PCI/DTB 枚举、DeviceGrant、DeviceAllocation、驱动启动与重启。
- virtio 驱动：blk、net、input、rng、console；MMIO、DMA、IRQ 走 HMP 与 DeviceGrant。
- DMA/IOMMU：Dma Capacity、Frame pin、IOMMU 域与缓存维护。
- VFS：挂载表、DocId/集合/修订、FileHandle、页缓存、路径与 fd 兼容服务。
- HolyFS：文档、修订、集合、标签、快照、COW、checksum、压缩。
- 存储后端：ext4、tmpfs、devfs、procfs。

### 退出标准

1. ext4 与 HolyFS 挂载、读写、重新挂载落盘校验通过。
2. VFS/HolyFS 重启后恢复；在途请求返回明确错误。
3. virtio-blk/net/input/rng/console 冒烟通过。
4. 1 MiB 顺序读达到预算；共享 Frame 零拷贝路径有测试。
5. `vfs`、`holyfs`、`restart` conformance 通过。

---

## 7. M9 — 原生 UI 与工具链

### 交付

- Display Server：帧缓冲、窗口、合成器、焦点、z 序、surface/seat。
- 指针系统：悬停命中、光标形状、双击最大化、拖拽与缩放。
- Console：终端网格、UTF-8、中文双宽、SGR、光标与提示符。
- Input Driver：virtio-input 键盘与绝对指针、按钮、滚轮。
- HolyDoc：解析、布局、渲染、链接、`@exec`、文档浏览。
- Workbench：编辑、浏览、构建、运行、调试。
- Toolchain Server：系统内编译、链接、打包、缓存；`H:/Apps` 构建到 `H:/System/Apps`；Workbench 的构建与运行走同一服务；用户看到“构建文档”，链接步骤由服务隐藏。
- Checkpoint Server：Native Task 快照与恢复；寄存器、Arena、用户页与 TCB 重建；`H:/Checkpoints` 分块落盘与读回。
- Debug Server：Task/资源视图、trace、符号化回溯。
- Sprite/Font 编辑器：原生图形与字体资产编辑。
- Home/Once 启动文档：Shell 启动执行 `H:/Home`；`H:/Once` 首次执行后写 `Once.done`。
- 字体与主题：可缩放字体、位图回退、深/浅主题、界面缩放。

### 退出标准

1. QEMU 启动到图形 Console 与 HolyShell。
2. 多窗口、焦点、键盘、指针、拖拽与缩放正常。
3. HolyDoc 链接跳转与 `@exec` 结果嵌入。
4. Display 崩溃后重启，Console 重连。
5. 全屏重绘达到预算；UI 冒烟全绿。
6. Toolchain Server 从 `H:/Apps` 构建应用到 `H:/System/Apps`；Workbench 构建、运行与调试闭环通过。
7. Checkpoint 保存/恢复与磁盘快照通过；Home/Once 启动文档执行通过。

---

## 8. M10 — 兼容

### 交付

- Compatibility Server：按应用选择 POSIX、Linux、Harmony personality，管理 rootfs、命名空间与配额。
- POSIX：POSIX Server、musl、`hk-abi`/`hk-crt`、fork/exec/wait、文件、线程、信号、管道、socket。
- Linux ABI：Linux personality、ELF/`PT_INTERP`、glibc/musl 动态加载、`/proc`、vDSO、Wayland/XWayland。
- Harmony：OpenHarmony 用户态、KAL/HDI display/input/storage/network 适配。
- POSIX 工具链：`holybuild`、sysroot、CMake/Meson/Autotools cross toolchain；与 Toolchain Server 并存。
- `hk-compat`：测试矩阵、syscall 覆盖表与真实应用回归。

### 退出标准

1. POSIX shell、coreutils、pthreads、信号、pipe、socket 测试通过。
2. 静态与动态 musl/glibc BusyBox 与基础 coreutils 运行。
3. `/proc/self/maps`、auxv、vDSO、TLS 语义正确。
4. Wayland `wl_shm` 与一个 X11 客户端经 XWayland 显示到 Holy surface。
5. Harmony 最小 ArkUI 应用通过 HDI 显示到 Holy surface。

---

## 9. M11 — 1.0 与多架构

### 交付

- AArch64/RISC-V 平台与全栈服务。
- Realtime 调度类与 Tempo 准入控制；Lending 的跨安全域策略。
- 四目标 conformance：`boot`、`ipc`、`memory`、`irq`、`vfs`、`net`、`holyfs`、`posix`、`linux-abi`、`restart`。
- 性能基准、预算复核与回归门禁。
- 服务器重启矩阵与故障注入。
- 硬化：fuzz、长时间压力、`unsafe` 审计、依赖审查。
- 发布镜像：x86_64 ISO、AArch64/RISC-V 镜像、Hosted 模拟器。

### 退出标准

1. 所有里程碑退出标准成立。
2. 四目标 conformance 全绿；x86_64 UEFI 真机启动到 HolyShell。
3. 72 小时 SMP + 服务器重启 + VFS 故障注入：零死锁、零数据损坏、零未处理 panic。
4. POSIX 与 Linux ABI 测试套件达到公开目标。
5. 构建可复现；发布镜像与校验和齐全。

---

## 10. 风险与应对

| 挑战 | 影响 | 应对 |
| --- | --- | --- |
| HMP 对象面收敛 | 服务与客户端同步改动 | 共享契约 crate、对象面冻结、单测与冒烟门禁 |
| Capacity/Frame 实现 | 物理内存碎片与回收 | 全内存区帧库、引用计数、连续分配、压力测试 |
| Region 物化 | 多架构页表差异 | 页表机制包装、平台后端、memory conformance |
| Provider 缺页 | IPC 开销 | 内核快速路径、批量、预读与 fault token |
| 调度契约与优先级反转 | 交互延迟与服务饥饿 | Cadence/Meter、Tempo 准入、caller-pays Lending、确定性回放 |
| 平台与引导 | 范围扩大 | x86_64 先行、AArch64 紧随、Limine/UEFI 双后端 |
| 设备与存储一致性 | 数据损坏 | 单元 + 故障注入 + 后端重启测试 |
| UI 与兼容 | 依赖底层成熟 | 底层 conformance 通过后接入 |
| 内核可信基 | 安全风险 | 最小内核、句柄模型、`unsafe` 审计 |

---

## 11. 当前状态

- M0–M5 已完成；当前焦点是 **M6 底层收敛**：HMP 对象面、Capacity 帧库、Region 物化、COW、MMIO/DMA 与 HMP ABI。
- 实现现状（设计契约见 [HMP](hmp.md)）：单一物理后端与 Capacity 配额视图；System Capacity 覆盖除 DMA 预留外的全部可用内存，内核堆、页表、镜像/栈/Arena 页计入 System；Root 建立只预切一个页窗口、按分配从 System 动态增长的顶层 User Capacity，Memory Server 共享该容量，匿名 capability、Frame 与 Provider 页从该用户账出页；Arena 以惰性匿名 Region 建立，装载期不预映射，`SYS_ALLOC` 按页推进并计入任务 Ledger；Capacity `used` 只计已分配 Frame，merge 失败由后端返回，多窗口 split 从尾部空闲的窗口切出；`SYS_CAPACITY_INFO` 64 字节（含 peak/soft），`SYS_LEDGER_INFO` 48 字节（含 resident/soft），`SYS_LEDGER_ENUM` 按索引枚举容量 Ledger，每条约 64 字节（含 CapacityId/用途/用量/配额/峰值/压力/常驻/软上限）。
- 按设计继续收口：镜像与栈页物化为 Region、每个 Capacity 建立显式 Ledger、软上限策略接到 Root/Tempo。
- Region 的 `GROW`/`PINNED` 已接入运行时，并提供 `SYS_REGION_GROW`；Arena 内部惰性 Region 已进入 HMP Arena 表；Arena、Region、类型化 Capability 与 Frame 已有各自的关联 Ledger；对象账本可按句柄经 `SYS_LEDGER_INFO` 查询。
- 调度以 [Holy Cadence Model](scheduling.md) 为唯一契约；Cadence/Meter、Tempo、Beat 与 Lending 随 M6 落地。
- 已落地原型资产：Display、Console、Input、HolyDoc、Workbench、Debug、Checkpoint、Task Manager、HolyFS、POSIX 兼容层；按 M7–M10 继续深化。
- 当前代码分层与清单见 [Crate 分层](crates.md)；构建、运行与冒烟入口见 [工程策略](engineering.md)。

---

## 12. 相关文档

- [章程](charter.md)
- [总体架构](architecture.md)
- [内存模型（HMP）](hmp.md)
- [执行模型](execution.md)
- [服务器](servers.md)
- [VFS](vfs.md)
- [HolyFS 文件系统](holyfs.md)
- [平台与启动](platforms.md)
- [工程策略](engineering.md)
