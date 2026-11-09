# Crate 分层与依赖规则

状态：**现行** · 上级：[总体架构](architecture.md) · 相关：[工程策略](engineering.md)

本文是工作区 crate 的权威清单与依赖规则。规则由 `cargo xtask deps` 自动检查，并接入 `cargo xtask lint` 与 CI。

---

## 1. 分层

crate 按侧别划分：共享契约、内核侧与用户态。侧别同时决定目录（`crates/shared`、`crates/kernel`、`crates/user`）与依赖规则，层号只表示依赖深度。

| 层 | 侧别 | crate | 职责 |
| --- | --- | --- | --- |
| L0 | 共享契约 | `hk-base` | 地址、架构标识、启动信息、时间与用户地址布局；零依赖 |
| L0 | 共享契约 | `hk-catalog` | 服务名称、启动白名单、桌面排序与默认 Cadence 的单一清单；依赖 `hk-cadence` |
| L0 | 共享契约 | `hk-cadence` | Cadence/Meter 类型、调度类别、虚拟时间数学、亲和性、借贷与快照格式；依赖 `hk-base` |
| L0 | 共享契约 | `hk-syscall` | 系统调用编号与跨边界 ABI 编码；零依赖 |
| L0 | 共享契约 | `hk-status` | 服务状态枚举：协议、文件系统与 VFS 共用的状态词表；零依赖 |
| L0 | 共享契约 | `hk-device` | 设备授权描述、PCI 地址与配置访问契约、通用总线枚举；只依赖 `hk-base` |
| L0 | 共享契约 | `hk-mmio` | MMIO 寄存器窗口（`volatile-register` 包装）；内核设备页与用户态驱动共用 |
| L1 | 共享契约 | `hk-image`、`hk-checkpoint` | 原生任务镜像格式与服务角色元数据、检查点快照格式；零依赖 |
| L1 | 共享契约 | `hk-shell`、`hk-font`、`hk-symbols` | 命令框架（零依赖）；字体（内核取位图字形，用户态取可缩放栅格化）；文本符号表解析与符号化 |
| L0 | 内核侧 | `hk-hal`、`hk-handle`、`hk-ipc`、`hk-mem`、`hk-kernel` | 平台 trait、句柄、IPC、HMP 对象模型与页机制、TCB/调度与内核对象 |
| L1 | 内核侧 | `hk-boot`、`hk-acpi`、`hk-fw`、`hk-framebuffer`、`hk-smp`、`hk-test`、`hk-drivers` | 引导后端（Limine/DTB）与启动信息、ACPI、固件描述（ACPI/DTB 包装）、帧缓冲文本控制台、SMP 状态机、测试退出、16550 UART 与 PS/2 键盘 |
| L4 | 内核侧 | `hk-runtime`、`hk-root-server`、`hk-kernel-shell`、`hk-sim` | 用户任务视图与系统调用分派、镜像装载与服务表、内核态 Shell 宿主、确定性仿真 |
| L4 | 内核侧 | `hk-platform-hosted`、`hk-platform-x86_64`、`platforms/*` | Hosted 模拟与 x86_64 硬件适配 |
| L2 | 用户态 | `hk-syscall-client` | 零分配系统调用薄客户端：参数整理、返回值解包与 Root 控制；并发库直接使用 |
| L2 | 用户态 | `hk-server-api` | 内核提供给用户态服务的通用面：HMP、Root 控制、设备授权、帧缓冲与运行时样板；系统调用薄封装重新导出 `hk-syscall-client` |
| L2 | 用户态 | `hk-proto` | 服务线协议、公共帧编解码、状态词表与协议标签区间 |
| L2 | 用户态 | `hk-proto-client` | Holy/VFS 协议客户端；收发缓冲在首次请求时按需分配 |
| L2 | 用户态 | `hk-block`、`hk-block-ipc`、`hk-vfs`、`hk-fs`、`hk-holy-proto`、`hk-holy-service`、`hk-fs-remote`、`hk-fs-ext4`、`hk-fs-holyfs`、`hk-fs-holyfs-vfs` | 块契约与内存盘、块服务 IPC 运行时、命名空间、文件系统契约、原生文档响应层、服务端分派、远程后端适配与实现 |
| L2 | 用户态 | `hk-net`、`hk-driver-virtio`、`hk-driver-ahci`、`hk-driver-host`、`hk-driver-rdif`、`hk-canvas`、`hk-worker`、`hk-thread`、`hk-sync`、`hk-async`、`hk-coroutine`、`hk-debug` | 网络协议栈、virtio/AHCI 驱动适配、外部驱动 crate 的 DMA/MMIO/log 宿主层与 RDIF 块桥、像素绘制、工作线程与并发库、符号化与回溯 |
| L2 | 用户态 | `hk-server-build` | 服务链接脚本（构建期） |
| L3 | 用户态 | `servers/*`、`clients/*`、`demos/*` | 常驻服务、自检客户端与演示服务；`servers/tempo` 承载 Cadence 策略与准入 |

`hk-shell`、`hk-syscall`、`hk-status` 与 `hk-block` 必须保持零依赖：`hk-shell` 通过 `ShellHost::extension` 把用户态命令交给宿主；`hk-syscall` 只是内核与用户态共用的编号与编码，不承载服务协议或服务实现；`hk-block` 只定义块设备契约与内存盘，IPC 运行时放在 `hk-block-ipc`。

---

## 2. 依赖规则

| 侧别 | 允许 | 禁止 |
| --- | --- | --- |
| 共享契约 | 更底层的共享契约 crate | 内核侧、用户态 |
| 内核侧 | 内核侧、共享契约 | 用户态 |
| 用户态 | 用户态、共享契约 | 内核侧 |
| 宿主工具（`xtask`） | 任意（构建期） | — |

侧别由目录决定：`crates/shared`、`crates/kernel`、`crates/user`。直接放在 `crates/` 下或未放入这三个目录的 crate 会让 `cargo xtask deps` 失败。`hk-shell`、`hk-syscall` 与 `hk-block` 另外要求零依赖；块服务的线协议运行时在 `hk-block-ipc` 的 `client` 与 `server` 模块，客户端与服务端可分别测试；直写缓冲区登记属于各客户端自身状态，容量可配置，满时返回错误而不是静默丢弃。

协议标签统一按 `0x100` 分段：基址只在 `hk-proto/src/labels.rs` 登记，各协议常量由基址派生，`LABEL_GROUPS` 登记全部服务协议标签（virtio 标签由驱动 crate 按 `labels::VIRTIO` 派生并自检）并由测试检查分段与全局唯一；Memory Server 与 Provider 缺页协议分别占 `0x1100`、`0x1200` 段。句柄权利使用 `RIGHT_*`，Holy 文档/卷令牌权利使用 `TOKEN_RIGHT_*`，两套按域命名。

在侧别规则之上还有几条窄约束，由同一命令检查：`hk-syscall-client` 只依赖 `hk-base` 与 `hk-syscall`，并发库（`hk-sync`、`hk-thread`、`hk-async`、`hk-worker`、`hk-debug`）只依赖它而不是完整 `hk-server-api`；`hk-server-api` 不得依赖 `hk-proto`；文件系统契约 `hk-fs` 只依赖共享契约；HolyFS 原生实现 `hk-fs-holyfs` 不依赖 `hk-vfs` 或其适配器；服务端 Holy 分派 `hk-holy-service` 与远程后端适配 `hk-fs-remote` 都不依赖服务二进制；`hk-proto` 只依赖内核面与 ABI，不引入会要求全局分配器的 `hk-fs`（无堆服务同样链接 `hk-proto`），原生文档的共享响应层放在 `hk-holy-proto`。

HMP 是唯一内存契约。对外面包含 `Arena / Capacity / Frame / Region / Capability / Provider / Ledger / Mappable / Handle / Rights`；页表、固件解析、引导与架构库位于包装 crate 内部。

服务之间只通过 Endpoint、Handle 与 `hk-proto` 的线协议协作，不链接彼此的实现。

字体统一由 `hk-font` 提供：内核帧缓冲控制台以 `default-features = false` 只取位图字形，用户态服务启用 `scalable` 取得 TrueType 栅格化。`xtask` 因此按目标把内核与服务分成两组构建，避免 Cargo 把特性取并集后把字体数据链进内核。像素绘制与文本落位统一走 `hk-canvas`，它直接以 `hk-font::Font` 为字体来源：调用方把字体交给 `draw_text`、`draw_glyph` 与 `text_width`，不需要在两者之间做适配。`hk-font` 常驻 `default_font`（16 像素可缩放）、`default_font_scaled`（100%/200% 界面缩放）与 `default_monospace`（16 像素等宽）三份默认字体，`hk-canvas` 将它们重新导出；`draw_text_default`、`draw_glyph_default` 与 `text_width_default` 无需传入字体即可排版。Display、Workbench、Debug 与 HolyDoc 正文使用可缩放默认字体，Console 字符网格与 HolyDoc 代码块使用等宽默认字体。同一 crate 还提供圆角矩形、椭圆、线段、水平/垂直/任意方向渐变、最近邻缩放拷贝、半透明拷贝与区域滚动；圆角与椭圆的抗锯齿使用整数定点运算，热路径不依赖浮点数学库。

文件系统遵循“契约 + 实现 + 后端”约定：`hk-fs` 定义 `DocId`、集合、修订、能力与 `Backend`；`hk-fs-holyfs` 是原生实现，只暴露文档操作，不得依赖 `hk-vfs`；`hk-fs-holyfs-vfs` 把原生实现接到契约上，不引入路径语义。`hk-vfs` 的核心也使用 `hk-fs::Backend`，路径后端通过 `PathObject` 适配。直接包装外部库且只有后端形态的文件系统（如 `hk-fs-ext4`）保留单一 crate。部署上 ext4 与 HolyFS 是对等的独立服务：`hk-server-ext4` 与 `hk-server-holyfs` 各自挂载块设备，共用 `hk-holy-service` 的盘符注册与请求分派；VFS 只做盘符聚合，Path 通过 `hk-fs-remote` 把远程 Holy 后端当作普通 `hk-fs::Backend`。tmpfs/devfs/procfs 仍由 VFS 进程内建。

目录约定：共享契约、内核侧与用户态库分别放 `crates/shared/`、`crates/kernel/`、`crates/user/`；常驻服务放 `servers/`，只通过协议访问服务的自检/状态客户端放 `clients/`，内核能力演示与冒烟服务放 `demos/`；包名仍以 `hk-server-*` 命名，便于镜像与白名单按名称引用。`hk-server-api` 只承载内核提供的用户态服务面：HMP 能力与 Region、设备授权、帧缓冲、堆、共享输出格式化与运行时样板；系统调用薄封装与 Root 控制在 `hk-syscall-client`，并由 `hk-server-api` 重新导出给服务。服务线协议与帧编解码在 `hk-proto` 的各服务模块（公共帧在 `wire.rs`，状态码在 `status.rs`，标签区间在 `labels.rs`）；需要堆缓冲的 Holy/VFS 客户端在 `hk-proto-client`，收发缓冲按需分配，`hk-proto` 本身保持零分配。各层都从 `hk-syscall` 只显式引用用到的 ABI 常量与类型，不整体展开内核命名空间。

运行时与 Root Server 分层：`hk-runtime` 只负责用户任务视图、系统调用语义、Arena 与停泊服务线程的创建；`hk-root-server` 在其上实现镜像装载、服务表、监督/重启与 `RuntimeHost`。平台只依赖两者提供的宿主接口。

基础类型分层：`hk-base` 零依赖，定义地址、架构、启动信息、时间与用户地址布局；`hk-hal` 在其上定义平台 trait 并重导出这些类型，`hk-device` 承载两侧共用的设备描述与 PCI 枚举，`hk-boot` 直接在 `hk-base` 之上实现 Limine 适配，避免 HAL 与引导适配互相依赖。

---

## 3. 单一事实来源

| 内容 | 位置 |
| --- | --- |
| 系统调用编号与跨边界 ABI 编码 | `crates/shared/hk-syscall/src/lib.rs` |
| 类型化能力判类（`CapabilityKind`） | `crates/shared/hk-syscall/src/lib.rs`，内核 `hk-mem` 与用户态 `hk-server-api` 共用 |
| 句柄权利与 Region 标志位 | `crates/shared/hk-syscall/src/lib.rs`（`RIGHT_*`、`REGION_FLAG_*`），`hk-handle` 与 `hk-mem` 直接引用；文档/卷令牌权利在 `hk-proto::holy` 的 `TOKEN_RIGHT_*` |
| HMP 对外词汇、Rights 与授予面 | [HMP 文档](hmp.md) 与 [ADR-0016](adr/0016-hmp-native-contract.md) |
| 调度契约、Cadence/Meter 词汇与授予面 | [Holy Cadence Model](scheduling.md) 与 [ADR-0017](adr/0017-holy-cadence.md)；共享类型 `hk-cadence`，服务默认契约在 `crates/shared/hk-catalog/src/lib.rs` |
| 服务角色元数据与 `.hk_service` 段 | `crates/shared/hk-image/src/role.rs` |
| 用户虚拟布局与自动映射窗口 | `crates/shared/hk-base/src/user.rs` |
| 时间类型（Moment、Duration、CivilTime） | `crates/shared/hk-base/src/time.rs` |
| 设备授权描述与 PCI 枚举 | `crates/shared/hk-device/src/{device,grant,pci,scan}.rs` |
| MMIO 寄存器访问（volatile-register 包装） | `crates/shared/hk-mmio/src/lib.rs` |
| 固件描述（ACPI/DTB 外部库包装） | `crates/kernel/hk-fw/src/{acpi,dtb}.rs`；`acpi`、`hermit-dtb` 只在此 crate 内出现 |
| 引导后端（Limine 多架构/DTB）与 BootInfo 转换 | `crates/kernel/hk-boot/src/{lib,limine,dtb}.rs`；crate 根只定义 `BootBackend`/`BootError`，平台显式选择后端 |
| x86_64 HAL `Platform`、CPUID 特性矩阵与熵源 | `crates/kernel/hk-platform-x86_64/src/{platform,entropy}.rs`、`cpu/features.rs` |
| 设备中断向量基址 | `crates/shared/hk-base/src/irq.rs` |
| HMP 核心对象（Arena/Capacity/Frame/FrameLease/Capability/Region/Provider/Ledger） | `crates/kernel/hk-mem/src/hmp/`；对外词汇与授予规则见 [ADR-0016](adr/0016-hmp-native-contract.md) |
| 物理页后端与按容量归因的额度视图 | `crates/kernel/hk-mem/src/hmp/backend.rs`、`crates/kernel/hk-mem/src/hmp/capacity.rs` |
| 系统物理帧银行、按用途页池、页表机制与 Arena Region 登记 | `crates/kernel/hk-mem/src/page/` |
| Dma Capacity 与软件 IOMMU | `crates/kernel/hk-mem/src/platform/` |
| 像素绘制、形状与渐变、缩放拷贝、文本落位与字宽累计 | `crates/user/hk-canvas/src/{surface,color,rect,text}.rs` |
| 字形度量、栅格化、缓存与内置字体数据 | `crates/shared/hk-font/src/{font,glyph,raster,cache,data}.rs` |
| x86_64 页表项属性（`page_table_entry`）、页表机制与写时复制 | `crates/kernel/hk-platform-x86_64/src/paging/{flags,table,cow}.rs` |
| Holy 文件系统契约与后端接口 | `crates/user/hk-fs/src/backend.rs` |
| 文档与卷级令牌会话（复用 `TokenTable`） | `crates/user/hk-fs/src/session.rs` |
| 系统调用薄封装、Root 控制与架构入口 | `crates/user/hk-syscall-client/src/{syscall,root,arch}.rs` |
| 服务线协议与编解码 | `crates/user/hk-proto/src/*` |
| Holy/VFS 协议客户端（缓冲按需分配） | `crates/user/hk-proto-client/src/{holy,vfs}.rs` |
| Holy 文档响应构造与错误映射 | `crates/user/hk-holy-proto/src/response.rs` |
| 协议公共帧头（请求/响应） | `crates/user/hk-proto/src/wire.rs`；块设备响应在头后追加固定 `tag` |
| 服务状态枚举 | `crates/shared/hk-status/src/lib.rs`；协议常量、`hk_fs::Error::status()` 与 `VfsError::status()` 都从它派生 |
| Holy 服务端盘符注册与请求分派 | `crates/user/hk-holy-service/src/{drives,handle}.rs`；VFS 与 ext4 服务共用 |
| 远程 Holy 后端适配 | `crates/user/hk-fs-remote/src/lib.rs`；VFS 与 Path Server 共用 |
| ext4 独立服务入口 | `servers/ext4/src/main.rs`；挂载 `E:` 并进入统一 Holy 请求循环 |
| 服务自检入口 | `servers/*/src/selftest.rs`；net、debug、taskman、console、display、memory、holydoc、workbench，`_start` 只做选择与转交 |
| 服务响应封装 | `crates/user/hk-proto/src/status.rs`；VFS、Holy 与块设备共用同一套状态值（含 `RANGE`/`BUSY`） |
| 协议标签基址与登记表 | `crates/user/hk-proto/src/labels.rs`（`LABEL_GROUPS` 含各服务、memory 与 provider 段；virtio 标签在驱动 crate 内派生并自检） |
| 服务协议标签与服务名常量 | `crates/user/hk-proto/src/*`；服务名清单在 `hk-catalog`，内存授权只按 Handle/Rights |
| 内核符号表与异常现场 | `crates/kernel/hk-root-server/src/debug/{symbols,fault}.rs`；文本符号解析与符号化在 `hk-symbols` |
| 用户态符号查询与回溯格式化 | `crates/user/hk-debug/src/lib.rs` |
| 检查点快照格式 | `crates/shared/hk-checkpoint/src/lib.rs` |
| 镜像服务清单、启动白名单与桌面排序 | `crates/shared/hk-catalog/src/lib.rs` |
| 服务链接基址 | `crates/user/hk-server-build/src/lib.rs`（所有服务共享同一镜像区） |
| 默认启动服务白名单读取 | `crates/kernel/hk-root-server/src/loader.rs` 读取 `hk-catalog` |
| 设备类与 DeviceGrant 协议 | `crates/user/hk-server-api/src/device.rs` 与 `crates/shared/hk-device/src/device.rs` |
| 平台启动特性 | 启动命令行 token（见各平台 loader） |

新增服务时只需要改 `crates/shared/hk-catalog/src/lib.rs` 的 `SERVICES` 与平台模式过滤，不再维护多份并行清单。

---

## 4. 新增服务清单

1. `servers/<name>/Cargo.toml`（客户端放 `clients/<name>/`）：只依赖 `hk-server-api`、`hk-proto` 与所需 L2 库；`[build-dependencies] hk-server-build`。
2. `build.rs`：`hk_server_build::configure()`；所有服务共享同一镜像区（各自独立 Arena）。
3. `src/main.rs`：
   - `#![cfg_attr(target_os = "none", no_std)]` 与 `no_main`；
   - 实现 `_start(endpoint)`；
   - 文件末尾调用 `hk_server_api::server_boilerplate!();`；
   - 服务发现使用 `hk_server_api::runtime::wait_service`。
4. 加入 `crates/shared/hk-catalog/src/lib.rs` 的 `SERVICES`；`boot` 决定默认启动，`desktop_order` 决定桌面顺序。
5. 平台 loader 的模式过滤直接读取同一目录的名称，不再维护独立启动白名单。
6. 服务线协议与标签放入 `hk-proto`，并在 `src/labels.rs` 的区间表里占一段；内核与用户态共用的编号与 ABI 编码放入 `hk-syscall`，不放入 `hk-shell`。
7. 为可自动验证的行为新增 `xtask smoke` 模式。

---

## 5. 检查

```sh
cargo xtask deps   # 依赖方向
cargo xtask lint   # deps + fmt + clippy
```

CI 的 `deps` 任务与 `clippy` 并行执行；违反规则时列出 crate、依赖与清单路径。
