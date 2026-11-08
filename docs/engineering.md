# 工程策略

状态：**设计中** · 上级：[总体架构](architecture.md)

本文定义 HolyKernel 的仓库布局、工具链、构建、CI、测试、可观测性、`unsafe` 审计、依赖审查与发布。

---

## 1. 仓库布局

```text
HolyKernel/
├── Cargo.toml / rust-toolchain.toml
├── xtask/                 构建、镜像、QEMU、smoke、文档、依赖检查
├── crates/
│   ├── shared/            共享契约
│   │   ├── hk-base（地址、架构、启动信息、时间、用户布局）
│   │   ├── hk-catalog（服务清单、启动白名单、桌面排序）
│   │   ├── hk-cadence（规划：Cadence/Meter、调度类别、虚拟时间数学）
│   │   ├── hk-syscall hk-device hk-image hk-checkpoint
│   │   └── hk-shell（零依赖） hk-font（位图给内核、可缩放给用户态） hk-symbols
│   ├── kernel/            内核侧
│   │   ├── hk-hal hk-handle hk-ipc hk-mem hk-kernel
│   │   ├── hk-boot hk-acpi hk-framebuffer hk-smp hk-test hk-drivers
│   │   ├── hk-runtime（用户任务、syscall 分派、RootServer 原语）
│   │   ├── hk-root-server hk-platform-x86_64 hk-platform-hosted
│   │   └── hk-kernel-shell hk-sim
│   └── user/              用户态库
│       ├── hk-server-api hk-syscall-client hk-proto hk-proto-client hk-block hk-block-ipc
│       ├── hk-holy-proto
│       ├── hk-net hk-fs hk-vfs
│       ├── hk-fs-ext4 hk-fs-holyfs hk-fs-holyfs-vfs
│       └── hk-driver-virtio hk-canvas hk-worker hk-thread hk-sync
│           hk-async hk-coroutine hk-debug hk-server-build
├── platforms/             x86_64-qemu、hosted
├── servers/               常驻用户态服务（含规划中的 tempo）
├── clients/               网络状态与 `*-test` 自检客户端
├── demos/                 demo/echo/fault/fork/latency/spin 演示服务
├── scripts/               随镜像打包的 HolyShell 脚本
└── docs/
```

完整 crate 清单、分层与规则见 [Crate 分层](crates.md)。

规则：

- 内核 crate 使用 `no_std`；服务器 crate 可以使用 `std`（Hosted）。
- 内核与服务堆使用可回收分配器（`linked_list_allocator`）；服务堆按需从 Arena 初始化，内核堆由 FrameBank 连续帧供给，启动早期使用 bootstrap 小堆。
- FrameBank 的每次分配生成 HMP `Ledger` 记账，并按 KernelHeap、PageTable、Cow、Restore、Image、Dma、Other 分类；输出 total/used/peak 与分类用量，启动日志打印 System Capacity 快照。
- 大块协议缓冲放在堆上，服务栈只保留小对象；IPC 单消息上限 64 KiB。
- 每个 crate 属于一个侧别：共享契约、内核侧或用户态，分别放在 `crates/shared`、`crates/kernel`、`crates/user`。内核侧只依赖内核侧与共享契约，用户态只依赖用户态与共享契约，共享契约只依赖更底层的共享契约；规则由 `cargo xtask deps` 强制。
- 平台差异位于 `hk-platform-*` 与 `platforms/`。
- 平台二进制保留入口、引导适配与用户现场切换；用户运行时、系统调用分派与启动装配位于 `crates/`。
- libc 提供 C ABI；内核与服务器使用 Rust。

---

## 2. 工具链

- 固定 `stable` 工具链，写入 `rust-toolchain.toml`；裸机目标由 `rustup` 提供的预编译 `core` 支撑。
- 组件：`llvm-tools`、`rustfmt`、`clippy`；裸机三元组在 `rust-toolchain.toml` 的 `targets` 中声明。
- Edition：2024。
- 目标：`x86_64-unknown-none`、`aarch64-unknown-none`、host triple。
- 构建配置：`opt-level="s"`、`lto="thin"`、`overflow-checks=true`、`panic="abort"`。

### xtask

```sh
cargo xtask build --target hosted
cargo xtask build --target x86_64-qemu --release
cargo xtask image --target x86_64-qemu
cargo xtask run   --target x86_64-qemu
cargo xtask smoke --target x86_64-qemu
cargo xtask test  --target hosted
cargo xtask docs
cargo xtask deps
cargo xtask lint
cargo xtask budget
```

x86_64 QEMU 的 `run` 会自动创建 `build/run-disk.img` 并挂载为 virtio-blk；`cargo xtask disk` 可单独生成该磁盘。手动 QEMU 启动需要：

```sh
qemu-system-x86_64 -M q35 -m 256M -cdrom build/holy-x86_64.iso -serial stdio \
  -drive file=build/run-disk.img,if=none,id=disk,format=raw \
  -device virtio-blk-pci,drive=disk \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04
```

`exit` 与冒烟测试通过 `isa-debug-exit` 退出 QEMU；缺少该设备时内核安全停机。

图形桌面使用 `cargo xtask run --target x86_64-qemu --ui`，或给镜像传 `--cmdline desktop` 并附加 `virtio-keyboard-pci`/`virtio-tablet-pci`；该模式启动 Display、Console、Input 与 HolyDoc。`xtask run` 在存在 `/dev/kvm` 时自动启用 `-accel kvm -cpu host`，手动 QEMU 可同样加速。

当前 crate 清单与依赖规则见 [Crate 分层](crates.md)；`cargo xtask deps` 在 `lint` 与 CI 中强制检查。

产物：x86_64 ISO、AArch64/RISC-V 镜像、Hosted 可执行文件、服务器镜像、根文件系统镜像。

`xtask` 与 `hk-server-build` 是宿主构建期成员（`std`）；裸机构建按平台包选择目标，避免对整个 workspace 使用裸机 target。

---

## 3. CI

| 任务 | 内容 |
| --- | --- |
| `fmt` | `cargo fmt --check` |
| `clippy` | `cargo clippy -- -D warnings` |
| `deps` | `cargo xtask deps`：crate 依赖方向 |
| `host-test` | Hosted 单元测试 + 属性测试 |
| `build-matrix` | x86_64 / AArch64 / Hosted |
| `qemu-smoke` | 启动到 HolyShell |
| `qemu-arch` | AArch64/RISC-V smoke |
| `sim` | 确定性 IPC/调度/重启/故障注入 |
| `posix` | POSIX 测试套件子集 |
| `vfs` | ext2/tmpfs/devfs/procfs 测试 |
| `net` | virtio-net、TCP/UDP、DHCP、DNS、socket |
| `fuzz` | 解析器、IPC、VFS、ELF、网络协议 |
| `docs` | Markdown 链接、ADR 索引 |
| `deny` | 许可证、安全公告、重复依赖 |
| `budget` | 内核代码行数统计 |

---

## 4. 测试策略

### 金字塔

```text
        ┌──────────────┐
        │ QEMU + POSIX │  真实中断、驱动、VFS、POSIX 应用
        ├──────────────┤
        │ 确定性模拟   │  IPC、调度、服务器重启、故障注入
        ├──────────────┤
        │ Hosted 单元  │  内核对象、Handle、VFS、libc
        └──────────────┘
```

### 单元

- 内核：Task/TCB、Cadence/Meter、Beat、Arena、HandleTable、Endpoint、Notification、HMP（Capacity/Frame/Region/Capability/Provider/Ledger）。
- 服务器：Root/Tempo 状态机与 Cadence 授予、VFS/Path 缓存与路径、ext2、POSIX fd/信号。
- 网络：ARP/IPv4/IPv6、TCP 状态机、UDP、DHCP、DNS、socket 表。
- HolyFS：对象表、修订链、集合关系、allocator、checksum、COW 事务、压缩、快照、远程 VFS 后端。
- 原生体验：HolyDoc 表单、Workbench 服务调用、Toolchain 构建缓存、Checkpoint/restore、Debug 符号化。
- libc：musl 单元测试、`hk-abi` shim、`hk-crt` 启动/TLS、errno。

### 确定性模拟

- `hk-sim` 单线程、虚拟时钟、固定种子。
- 记录 IPC、调度、句柄、重启事件；失败输出可复现命令。
- 故障注入：IPC 超时、服务器崩溃、句柄回收、DMA 失败。

### QEMU + POSIX

- 启动到 HolyShell；通过 `posix` 运行 coreutils、shell 脚本、编译器。
- POSIX 测试套件子集；VFS 挂载与 ext2 读写。
- 服务器重启：驱动、VFS、POSIX、Network、HolyFS 崩溃后恢复。
- 网络：virtio-net + QEMU user-mode networking、ping、TCP、DNS。
- HolyFS：`H:` 盘符、文档读写、修订与快照、恢复。
- 基准：IPC、上下文切换、fork+exec、VFS 吞吐、网络吞吐、HolyFS 吞吐。

### fuzz

| 目标 | 内容 |
| --- | --- |
| `fuzz-ipc` | Endpoint 消息、Handle 传递 |
| `fuzz-vfs` | 路径、挂载、RPC |
| `fuzz-ext2` | 超级块、inode、目录、块映射 |
| `fuzz-elf` | ELF 头、段、重定位 |
| `fuzz-holydoc` | 文档解析、链接、表单 |
| `fuzz-toolchain` | 构建文档、编译期执行、资源生成 |
| `fuzz-checkpoint` | 快照序列、恢复、句柄重建 |
| `fuzz-net` | Ethernet、ARP、IPv4/IPv6、TCP、UDP、DHCP、DNS |
| `fuzz-holyfs` | superblock、对象表、修订记录、集合项、extent、压缩、文档名 |

---

## 5. 可观测性

- **trace**：每 CPU lock-free ring buffer；事件带时间戳、CPU、TCB、类别。
- **类别**：`boot`、`ipc`、`sched`、`mem`、`handle`、`vfs`、`net`、`posix`、`server`、`ui`。
- **服务器健康**：Root Server 记录服务状态、重启次数、最后错误。
- **panic**：停世界、dump 寄存器、栈、当前 TCB、Handle、最近 trace。
- **服务 panic**：`server_boilerplate!` 输出服务名、panic 内容与 `file:line:column`，随后以非零码退出；输出路径不分配内存。
- **调试**：`cargo xtask debug` 生成 `build/holy.gdb` 并以 `-S` 启动 QEMU；`cargo xtask inspect` 查看原生镜像的 Task/Region/能力/符号；`cargo xtask symbolize [--service <服务>] <地址>` 离线解析内核或服务地址（服务共享同一镜像区）；`hk-dbg` 串口协议；Hosted `--trace`/`--seed`。

---

## 6. 安全与信任模型

- **隔离**：内核与用户态分离；每个 Task 独立 Arena。
- **最小权限**：服务器只获得职责所需的 Handle。
- **句柄回收**：close + 任务退出 + 服务器重启；服务器重启回收旧句柄。
- **可信基**：内核、Root Server、VFS Server、根文件系统后端（ext4/HolyFS）；POSIX Server 在启用兼容环境时加入。
- **Rust**：内核与服务器使用 Rust；`unsafe` 最小化、注释、审计。
- **`unsafe` 规则**：每个块有 `// SAFETY:`；可变全局状态用 `Atomic*`/`SpinLock`/`Once`；裸指针集中在 MMIO/DMA/页表/上下文切换。
- **验证**：fuzz、确定性模拟、POSIX 测试、服务器重启测试。

---

## 7. 依赖策略

- 核心依赖使用 `no_std`；服务器可以使用 `std`。
- 许可证兼容：MIT/Apache-2.0/BSD/ISC/MPL-2.0。
- 依赖精简、可审计、维护活跃。
- `build.rs` 与过程宏经过审查。
- HMP 是唯一内存契约；对外面包含 `Arena / Capacity / Frame / Region / Capability / Provider / Ledger / Mappable`，页表、固件解析、引导与架构库在包装 crate 内部。
- 外部库按机制分类：页表机制、固件描述、UEFI 引导、架构原语、MMIO 寄存器、驱动协议栈；每类只由对应包装 crate 引入。
- `cargo-deny` 检查许可证与安全公告。

---

## 8. 发布

| 产物 | 平台 | 内容 |
| --- | --- | --- |
| `holy-x86_64.iso` | x86_64 BIOS/UEFI | 内核 + 服务器 + ext2/ext4 根镜像 + HolyFS 数据卷 |
| `holy-aarch64.img` | QEMU virt | 内核 + 服务器 + 根镜像 |
| `holy-riscv64.img` | QEMU virt | 内核 + 服务器 + 根镜像 |
| `holy-hosted` | Linux/macOS/Windows | 模拟器 |

- 发布附变更日志、已知限制、启动说明、校验和。
- 版本语义：`0.x` 设计/实验；`1.0` 满足 [路线图 M11](roadmap.md)。
- 构建可复现：固定工具链、锁定依赖、固定时间戳。

---

## 9. 代码评审清单

1. 变更与 [章程](charter.md) 的微内核模型对齐。
2. 内核只包含机制；策略位于服务器。
3. 新增 Handle 有最小权限与close/回收路径。
4. 新增 IPC 有超时、错误与断线处理。
5. 新增 `unsafe` 有 `// SAFETY:`。
6. 新增解析器/格式有 fuzz 目标。
7. 新增失败路径有测试（OOM、超时、EIO、服务器崩溃）。
8. 服务器崩溃有重启策略与状态恢复。
9. 文档、ADR、术语、链接已更新。

---

## 10. 保证

1. 内核 crate 可 `no_std` 构建。
2. CI 的 build matrix、sim、posix、vfs 任务持续开启。
3. 每个 `unsafe` 块有安全注释。
4. 每个解析器有 fuzz 目标。
5. 每个平台通过 conformance suite 后标记支持。
6. 基准预算达标是里程碑退出条件。
7. 构建可复现。
8. 系统由微内核与用户态服务器组成：Task/TCB、Cadence/Meter、Beat、Arena、HandleTable、Endpoint、Notification、HMP（Capacity/Frame/Region）、Handle。
9. 内核与用户态服务共享同一组 HMP 对象、Rights 与 Ledger 规则；内存授权由 Handle、Rights 与 ObjectKind 决定。

---

## 11. 相关文档

- [总体架构](architecture.md)
- [服务器](servers.md)
- [网络](network.md)
- [VFS](vfs.md)
- [HolyFS 文件系统](holyfs.md)
- [Holy 原生体验](experience.md)
- [POSIX 与 libc](posix.md)
- [平台与启动](platforms.md)
- [路线图](roadmap.md)
