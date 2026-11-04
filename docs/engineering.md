# 工程策略

状态：**设计中** · 上级：[总体架构](architecture.md)

本文定义 HolyKernel 的仓库布局、工具链、构建、CI、测试、可观测性、`unsafe` 审计、依赖审查与发布。

---

## 1. 仓库布局

```text
HolyKernel/
├── Cargo.toml                 workspace
├── rust-toolchain.toml        固定 stable 工具链
├── scripts/                  随镜像打包的 HolyShell 启动脚本（`.hsh`）
├── xtask/                     构建、镜像、QEMU、测试、文档
├── crates/
│   ├── hk-hal/                CPU/MMU/IRQ/Timer/SMP/Console/DMA/PCI 契约与 ECAM、Port/MMIO Io；现场与页映射
│   ├── hk-acpi/               ACPI RSDP/SDT 查找与表条目迭代（架构无关）
│   ├── hk-drivers/            通用设备协议：桥感知 PCI、16550 UART、PS/2 键盘
│   ├── hk-framebuffer/        通用帧缓冲文本控制台
│   ├── hk-smp/                通用 SMP 状态机：AP 池、TLB shootdown、空闲唤醒
│   ├── hk-kernel/             微内核：TCB/VSpace/HandleTable/Endpoint/Notification、trace；per-CPU 调度与线性堆
│   ├── hk-handle/                Handle、HandleTable、duplicate/close
│   ├── hk-ipc/                Endpoint、Notification、消息
│   ├── hk-mem/                Untyped、Frame、VSpace、COW、DMA、线性帧分配器、用户地址空间登记表
│   ├── hk-boot/               Limine/U-Boot/OpenSBI 适配与启动信息校验
│   ├── hk-platform-*          平台实现：端口 I/O、legacy PCI、分页、每任务页表登记、MADT、per-CPU TSS 与 IPI、键盘、引导初始化
│   ├── hk-root-server/        Root Server、服务清单、服务表、ELF 装载、用户运行时与运行时宿主
│   ├── hk-device-manager/     设备枚举与 DeviceGrant
│   ├── hk-driver-*            virtio/PS2/console/framebuffer
│   ├── hk-vfs/                VFS 核心：路径、挂载表、dentry/页缓存、文件句柄、tmpfs
│   ├── hk-fs-ext2/            ext2 后端
│   ├── hk-fs-tmpfs/           tmpfs 后端
│   ├── hk-fs-devfs/           devfs 后端
│   ├── hk-fs-procfs/          procfs 后端
│   ├── hk-posix-server/       POSIX Server
│   ├── hk-abi/                POSIX syscall shim 协议与头文件
│   ├── hk-crt/                启动、TLS、signal trampoline、ELF 加载
│   ├── hk-libc/               薄兼容层：HK 扩展与 ABI 胶水
│   ├── hk-syscall/            系统调用编号与共享 ABI 常量
│   ├── hk-server-api/         入口 ABI：调用约定与按架构的 int 0x80 / svc 入口
│   ├── hk-shell/              HolyShell 命令解析、输出、ShellIo/ShellHost、执行循环与脚本求值
│   ├── hk-server-build/       服务镜像链接配置（构建期生成链接脚本）
│   ├── third_party/musl/      musl 移植（兼容环境主 C libc）
│   ├── hk-network/            Network Server
│   ├── hk-display/            Display Server
│   ├── hk-holydoc/            HolyDoc Server
│   ├── hk-sim/                确定性模拟、故障注入
│   └── hk-test/               测试支撑、QEMU 退出协议
├── platforms/
│   ├── x86_64-qemu/
│   ├── aarch64-qemu/
│   ├── riscv64-qemu/
│   └── hosted/
├── servers/                   服务清单与根文件系统镜像
├── docs/
└── tools/
```

规则：

- 内核 crate 使用 `no_std`；服务器 crate 可以使用 `std`（Hosted）。
- 内核 crate 只依赖 `hk-hal` 与内核对象 crate；服务器通过 IPC 与 Handle 交互。
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
cargo xtask test-posix --target x86_64-qemu
cargo xtask bench --target hosted
cargo xtask docs
cargo xtask lint
cargo xtask budget
```

产物：x86_64 ISO、AArch64/RISC-V 镜像、Hosted 可执行文件、服务器镜像、根文件系统镜像。

`xtask` 与 `hk-server-build` 是宿主构建期成员（`std`）；裸机构建按平台包选择目标，避免对整个 workspace 使用裸机 target。

---

## 3. CI

| 任务 | 内容 |
| --- | --- |
| `fmt` | `cargo fmt --check` |
| `clippy` | `cargo clippy -- -D warnings` |
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

- 内核：TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Untyped。
- 服务器：Root Server 状态机、VFS 路径/缓存、ext2、POSIX fd/信号。
- 网络：ARP/IPv4/IPv6、TCP 状态机、UDP、DHCP、DNS、socket 表。
- HolyFS：extent tree、allocator、checksum、COW 事务、压缩、快照、VFS/POSIX inode、权限与硬链接。
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
- HolyFS：`H:` 挂载、HolyFS 根启动、POSIX 文件操作、快照与恢复。
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
| `fuzz-holyfs` | superblock、日志、extent、目录、inode、压缩、文档名 |

---

## 5. 可观测性

- **trace**：每 CPU lock-free ring buffer；事件带时间戳、CPU、TCB、类别。
- **类别**：`boot`、`ipc`、`sched`、`mem`、`handle`、`vfs`、`net`、`posix`、`server`、`ui`。
- **服务器健康**：Root Server 记录服务状态、重启次数、最后错误。
- **panic**：停世界、dump 寄存器、栈、当前 TCB、Handle、最近 trace。
- **调试**：QEMU `-s -S` + gdb；`hk-dbg` 串口协议；Hosted `--trace`/`--seed`。

---

## 6. 安全与信任模型

- **隔离**：内核与用户态分离；每进程独立 VSpace。
- **最小权限**：服务器只获得职责所需的 Handle。
- **句柄回收**：close + 任务退出 + 服务器重启；服务器重启回收旧句柄。
- **可信基**：内核、Root Server、POSIX Server、VFS Server、根文件系统 Server（ext2/ext4 或 HolyFS）。
- **Rust**：内核与服务器使用 Rust；`unsafe` 最小化、注释、审计。
- **`unsafe` 规则**：每个块有 `// SAFETY:`；可变全局状态用 `Atomic*`/`SpinLock`/`Once`；裸指针集中在 MMIO/DMA/页表/上下文切换。
- **验证**：fuzz、确定性模拟、POSIX 测试、服务器重启测试。

---

## 7. 依赖策略

- 核心依赖使用 `no_std`；服务器可以使用 `std`。
- 许可证兼容：MIT/Apache-2.0/BSD/ISC/MPL-2.0。
- 依赖精简、可审计、维护活跃。
- `build.rs` 与过程宏经过审查。
- `cargo-deny` 检查许可证与安全公告。

---

## 8. 发布

| 产物 | 平台 | 内容 |
| --- | --- | --- |
| `holy-x86_64.iso` | x86_64 BIOS/UEFI | 内核 + 服务器 + ext2/ext4 或 HolyFS 根镜像 |
| `holy-aarch64.img` | QEMU virt | 内核 + 服务器 + 根镜像 |
| `holy-riscv64.img` | QEMU virt | 内核 + 服务器 + 根镜像 |
| `holy-hosted` | Linux/macOS/Windows | 模拟器 |

- 发布附变更日志、已知限制、启动说明、校验和。
- 版本语义：`0.x` 设计/实验；`1.0` 满足 [路线图 M9](roadmap.md#12-m9--10)。
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
8. 系统由微内核与用户态服务器组成：TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Handle。

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
