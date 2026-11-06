# Crate 分层与依赖规则

状态：**现行** · 上级：[总体架构](architecture.md) · 相关：[工程策略](engineering.md)

本文是工作区 crate 的权威清单与依赖规则。规则由 `cargo xtask deps` 自动检查，并接入 `cargo xtask lint` 与 CI。

---

## 1. 分层

| 层 | crate | 职责 |
| --- | --- | --- |
| L0 基础类型 | `hk-base` | 地址、架构标识、启动信息与校验；零依赖，HAL 与引导适配共用 |
| L0 内核 ABI 与对象 | `hk-syscall`、`hk-hal`、`hk-handle`、`hk-ipc`、`hk-mem`、`hk-kernel` | 系统调用编号、HAL 契约、句柄、IPC、内存、TCB/调度与内核对象 |
| L1 内核库 | `hk-boot`、`hk-acpi`、`hk-framebuffer`、`hk-smp`、`hk-image`、`hk-test` | 启动信息、ACPI、帧缓冲文本控制台、SMP 状态机、原生任务镜像、测试退出 |
| L2 用户态库 | `hk-server-api`、`hk-debug`、`hk-checkpoint`、`hk-coroutine`、`hk-async`、`hk-sync`、`hk-thread`、`hk-worker`、`hk-block`、`hk-font`、`hk-net`、`hk-fs`、`hk-vfs`、`hk-fs-ext4`、`hk-fs-holyfs`、`hk-fs-holyfs-vfs`、`hk-driver-virtio`、`hk-server-build` | 服务协议与客户端、用户态符号与回溯格式化、检查点快照格式、有栈协程、异步执行器、同步原语、线程库、工作线程池、块/文件系统核心、可缩放字体、网络协议栈、virtio 适配、服务链接脚本 |
| L3 服务 | `servers/*` | 常驻用户态驱动、文件系统、网络、Console、Display、Input |
| L3 客户端 | `clients/*` | netstat/netsock、`hk-server-dbg` 与 `*-test` 自检客户端；只通过协议访问服务 |
| L3 演示 | `demos/*` | demo/echo/fault/fork/latency/spin 等内核能力演示与冒烟服务 |
| L4 运行时 | `hk-runtime` | 用户任务视图、系统调用分派、Arena、RootServer 启动原语 |
| L4 内核 Shell | `hk-kernel-shell` | 内核态 Shell 宿主、运行时视图转换、服务启动/重启/脚本编排；只依赖 `hk-shell` 与 `hk-root-server` |
| L4 装配 | `hk-root-server`、`hk-platform-x86_64`、`hk-platform-hosted`、`platforms/*`、`xtask` | 服务表、ELF 装载、监督、运行时宿主、符号表与异常回溯、平台实现、镜像与测试工具 |
| 独立 | `hk-shell` | 零依赖纯库：命令框架与内核可见的核心命令 |

`hk-shell` 必须保持零依赖；用户态命令通过 `ShellHost::extension` 交给宿主，不以任何形式引入内核或服务实现。

---

## 2. 依赖规则

| crate 类别 | 允许 | 禁止 |
| --- | --- | --- |
| L0 内核 crate | `hk-base`、`hk-hal` 与内核对象 crate | `hk-root-server`、`hk-platform-*`、`hk-server-*`、`hk-net`、`hk-vfs`、`hk-fs-*`、`hk-block`、`hk-driver*`、`hk-framebuffer`、平台二进制 |
| L1 内核库 | `hk-hal` 与内核对象 crate | 同上实现层 crate |
| `hk-server-api` | `hk-hal`、`hk-syscall` | `hk-kernel`、`hk-runtime`、`hk-root-server`、`hk-platform-*`、其他服务 |
| `hk-debug` | `hk-server-api` | `hk-kernel`、`hk-runtime`、`hk-root-server`、`hk-platform-*` |
| `hk-runtime` | 内核 ABI/对象 crate | `hk-root-server`、`hk-server-*`、`hk-platform-*`、平台二进制 |
| `hk-root-server` | `hk-runtime`、内核与 L2 库 | `hk-server-*`、`hk-platform-*`、`hk-kernel-shell`、平台二进制 |
| `hk-kernel-shell` | `hk-shell`、`hk-root-server`、`hk-hal` | `hk-server-*`、平台二进制 |
| L2 用户态库 | `hk-hal`、`hk-syscall`、`hk-server-api`、同层库 | `hk-kernel`、`hk-root-server`、`hk-platform-*`、平台二进制 |
| L3 服务/客户端/演示 | `hk-server-api`、`hk-hal`、L2 库 | `hk-kernel`、`hk-root-server`、`hk-platform-*` |
| `hk-shell` | 无 | 任何依赖 |

服务之间只通过 Endpoint、Handle 与 `hk-server-api` 的线协议协作，不链接彼此的实现。

文件系统遵循“契约 + 实现 + 后端”约定：`hk-fs` 定义 `DocId`、集合、修订、能力与 `Backend`；`hk-fs-holyfs` 是原生实现，只暴露文档操作，不得依赖 `hk-vfs`；`hk-fs-holyfs-vfs` 把原生实现接到契约上，不引入路径语义。`hk-vfs` 的核心也使用 `hk-fs::Backend`，路径后端通过 `PathObject` 适配。直接包装外部库且只有后端形态的文件系统（如 `hk-fs-ext4`）保留单一 crate。

目录约定：常驻服务放 `servers/`，只通过协议访问服务的自检/状态客户端放 `clients/`，内核能力演示与冒烟服务放 `demos/`；包名仍以 `hk-server-*` 命名，便于镜像与白名单按名称引用。`hk-server-api` 的系统调用薄封装集中在 `src/syscall.rs`，共享输出格式化在 `src/format.rs`，协议与客户端在各子模块。

运行时与 Root Server 分层：`hk-runtime` 只负责用户任务视图、系统调用语义、Arena 与停泊服务线程的创建；`hk-root-server` 在其上实现镜像装载、服务表、监督/重启与 `RuntimeHost`。平台只依赖两者提供的宿主接口。

基础类型分层：`hk-base` 零依赖，定义地址、架构与启动信息；`hk-hal` 在其上定义平台 trait 并重导出这些类型，`hk-boot` 直接在 `hk-base` 之上实现 Limine 适配，避免 HAL 与引导适配互相依赖。

---

## 3. 单一事实来源

| 内容 | 位置 |
| --- | --- |
| 系统调用编号与线协议常量 | `crates/hk-syscall/src/lib.rs` |
| Holy 文件系统契约与后端接口 | `crates/hk-fs/src/backend.rs` |
| 服务客户端与编解码 | `crates/hk-server-api/src/*` |
| 用户态符号查询与回溯格式化 | `crates/hk-debug/src/lib.rs` |
| 检查点快照格式 | `crates/hk-checkpoint/src/lib.rs` |
| 镜像服务清单与装载顺序 | `xtask/src/catalog.rs` |
| 服务链接基址 | `crates/hk-server-build/src/lib.rs`（所有服务共享同一镜像区） |
| 默认启动服务白名单 | `crates/hk-root-server/src/loader.rs` |
| 设备类与 DeviceGrant 协议 | `crates/hk-server-api/src/device.rs` |
| 平台启动特性 | 启动命令行 token（见各平台 loader） |

新增服务时只需要改 `xtask/src/catalog.rs` 的 `SERVERS`、`BOOT_SERVICES` 与模式过滤，不再维护多份并行清单。

---

## 4. 新增服务清单

1. `servers/<name>/Cargo.toml`（客户端放 `clients/<name>/`）：只依赖 `hk-server-api` 与所需 L2 库；`[build-dependencies] hk-server-build`。
2. `build.rs`：`hk_server_build::configure()`；所有服务共享同一镜像区（各自独立 VSpace）。
3. `src/main.rs`：
   - `#![cfg_attr(target_os = "none", no_std)]` 与 `no_main`；
   - 实现 `_start(endpoint)`；
   - 文件末尾调用 `hk_server_api::server_boilerplate!();`；
   - 服务发现使用 `hk_server_api::runtime::wait_service`。
4. 加入 `xtask/src/catalog.rs` 的 `SERVERS`。
5. 如需默认启动，加入 `hk-root-server` 的 `BOOT_SERVICES`，并在平台 loader 的模式过滤中处理。
6. 协议与常量放入 `hk-server-api`，不放入内核或 `hk-shell`。
7. 为可自动验证的行为新增 `xtask smoke` 模式。

---

## 5. 检查

```sh
cargo xtask deps   # 依赖方向
cargo xtask lint   # deps + fmt + clippy
```

CI 的 `deps` 任务与 `clippy` 并行执行；违反规则时列出 crate、依赖与清单路径。
