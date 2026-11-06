# 术语表

状态：**设计中** · 上级：[总体架构](architecture.md)

本表定义 HolyKernel 的微内核、服务器与 POSIX 兼容层术语。

---

## 1. 微内核

| 术语 | 定义 |
| --- | --- |
| **HolyKernel (HK)** | 句柄型微内核 + 用户态服务器 + POSIX.1 兼容层 |
| **Microkernel** | 只提供 TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Untyped、IRQHandler 的内核 |
| **TCB** | 线程控制块；上下文、优先级、亲和性、状态 |
| **VSpace** | 进程地址空间；页表根、映射、保护 |
| **HandleTable** | 每 Task 的句柄表；槽位、generation、rights |
| **Handle** | index + generation；资源访问凭据 |
| **Rights** | READ、WRITE、EXECUTE、TRANSFER、ADMIN |
| **Epoch** | 对象撤销代际；`revoke_all` 递增，使旧 Handle 失效 |
| **Handle Quota** | 每 Task/服务器的 Handle 上限 |
| **DeviceGrant** | 设备句柄集合；MMIO/DMA/IRQ |
| **Endpoint** | 同步 IPC 通道；send/recv/call/reply、句柄传递 |
| **Notification** | 异步位通知；中断下半部、事件完成 |
| **Frame** | 物理内存帧；映射、共享、DMA |
| **Untyped** | 未类型化内存；retype 为内核对象 |
| **IRQHandler** | 中断句柄；绑定 Endpoint/Notification |
| **DeviceGrant** | 设备 MMIO/DMA/IRQ 句柄集合 |
| **Fault Endpoint** | 页错误与异常交付出口 |
| **Native Task** | 用户态一等 Task：VSpace + HandleTable + TCB(s) + Arena + 父子关系 |
| **Window Client** | Display Server 的客户端；可创建多个窗口 |
| **Arena** | Task 拥有的用户态堆；Task 退出时整体回收 |
| **Debug Server** | Task/资源视图、trace、符号化调试、`hk-dbg` |
| **Toolchain Server** | 系统内编译/链接/打包/缓存 |
| **Workbench** | 编辑/浏览/构建/运行/调试一体化原生应用 |
| **Checkpoint Server** | Native Task/Server 快照与恢复 |
| **Compatibility Server** | 按应用选择 POSIX/Linux/Harmony personality 的用户态 supervisor |
| **Personality** | 应用的运行时/ABI 域：POSIX、Linux 或 Harmony；各自独立 VSpace |
| **Surface / Seat** | 合成器表面与输入焦点/键盘/指针/剪贴板契约 |
| **Wayland / XWayland** | Linux Wayland 客户端与 X11 兼容桥接 |
| **Harmony Personality** | OpenHarmony 用户态 + Holy KAL/HDI 适配 |
| **HDI** | OpenHarmony 硬件驱动接口；Display/Input/Storage/Network 后端 |
| **FunSeg** | 地址到符号/源码的映射缓存，用于栈回溯与 HolyDoc 链接 |
| **Home / Once** | 可执行启动/首次运行文档 |

### 1.1 Holy Memory Plane（HMP）

| 术语 | 定义 |
| --- | --- |
| **HMP** | Holy Memory Plane；以 Region 统一映射、类型化能力承载身份的内存平面 |
| **Capacity** | Root Server 授予的物理容量能力；声明页大小、配额与用途 |
| **Region** | 类型化能力在 VSpace 中的唯一映射视图；带权利、COW、guard、pin 等标志 |
| **Provider** | 缺页、回写、驱逐的 Endpoint 服务；契约按类型定义 |
| **Ledger** | 容量、常驻、配额、压力的显式记账对象 |
| **类型化能力** | Revision、Surface、TaskImage、DeviceAllocation、Anonymous、Frame 各自以独立 Handle 类型暴露 |
| **Mappable** | 内核可见的统一能力接口；类型化能力实现它，由 Region 引用 |
| **Provider 契约** | 每类能力对应的 fault/commit/scanout/pin/export/swap 协议 |
| **TaskImage** | Native Task 镜像中的代码/数据区域；通常 sealed |
| **DeviceAllocation** | 显存/DMA 内存分配；MMIO 寄存器由 DeviceGrant 映射 |
| **Anonymous** | 由 MemoryManager 提供后备、页面初始为零的匿名内存 |
| **sealed** | 只读共享、拒绝写入的对象/Region 状态 |
| **pin** | 固定 Frame，保持驻留；用于 DMA、scanout |
| **provenance** | 内存对象的原生来源：Revision/Surface/TaskImage/Device/Anonymous |

## 2. 服务器

| 术语 | 定义 |
| --- | --- |
| **Root Server** | 启动服务器、分配 Handle、监督与重启 |
| **Device Manager** | 枚举 PCI/DTB、创建 DeviceGrant、启动驱动 |
| **VFS Server** | 挂载表、路径解析、dentry/inode 缓存、文件句柄、页缓存 |
| **FS Backend** | ext2/ext4、HolyFS、tmpfs、devfs、procfs 等文件系统服务器 |
| **HolyFS** | HolyKernel 原生文件系统；`H:` 盘符、文档、修订、集合、标签、快照 |
| **Document** | HolyFS 文档；DocId + head 修订 + 校验和；内容与集合项住在修订里 |
| **DocId** | HolyFS 文档的 128 位稳定身份 |
| **Revision** | HolyFS 文档的不可变版本；payload 承载内容 extent、集合项索引或两者 |
| **Collection** | HolyFS 文档的 Collection 修订；承载按名字索引的集合项 |
| **Tag** | 跨集合的文档成员关系 |
| **Snapshot** | COW 根引用；零拷贝、可回滚 |
| **POSIX Server** | 进程、fd、信号、pthreads、管道、Unix 套接字 |
| **Network Server** | 用户态 TCP/IP、UDP、DHCP、DNS、socket 表与策略 |
| **Socket** | 网络端点对象；READ recv、WRITE send、ADMIN bind/listen |
| **DHCP / DNS** | Network Server 内的地址租约与域名解析 |
| **TCP / UDP** | Network Server 内的传输协议 |
| **virtio-net** | 用户态网络驱动；通过 DeviceGrant 访问 NIC |
| **Display Server** | 帧缓冲、窗口、输入、真彩 UI |
| **Console Service** | POSIX 终端/控制台接口 |
| **HolyDoc Server** | 活文档解析、布局、渲染、链接 |
| **Service Manifest** | 服务名、原生镜像、Handle、依赖、重启策略 |

## 3. POSIX 与 libc

| 术语 | 定义 |
| --- | --- |
| **HolyShell** | 默认原生命令行与脚本环境；Holy 命令、结构化管道 |
| **HolyShell Session** | 一个 Native Task；拥有独立命名空间、历史、当前 Task、主题 |
| **Session Namespace** | 会话的命令、变量、别名与输出上下文 |
| **Holy 命令** | 通过 Handle/Endpoint 调用服务的原生命令 |
| **Command Registry** | 原生命令元数据与发现入口 |
| **POSIX Bridge** | `posix` 入口；启动 POSIX session 或执行 POSIX 命令 |
| **POSIX.1** | 兼容环境的应用接口标准；进程、文件、信号、线程、IPC |
| **musl** | POSIX 兼容环境的主 C libc；提供 POSIX 函数、stdio、malloc、pthreads |
| **应用自带 libc** | 应用静态链接的自有 C 运行时；必须实现 `hk-abi` 后端，非一等支持 |
| **hk-abi** | POSIX syscall shim 协议与头文件；把 musl 调用转换为 POSIX Server IPC |
| **hk-crt** | 启动代码、TLS、signal trampoline、ELF 加载 |
| **hk-libc** | 薄兼容层：HK 扩展与 ABI 胶水 |
| **syscall shim** | `hk-abi` 中把 POSIX 调用转换为 POSIX Server IPC 的层 |
| **PID** | POSIX 进程标识；由 POSIX Server 管理 |
| **fd** | POSIX 文件描述符；映射到 VFS FileHandle、pipe、socket 或 event |
| **OpenFile** | 打开文件描述；偏移、flags、引用计数 |
| **FileHandle** | VFS 维护的文件句柄；POSIX fd 表引用它 |
| **COW** | 写时复制；`fork` 与页错误处理 |
| **ELF Loader** | 兼容环境的 ELF 加载器；原生服务使用 HKX 镜像 |
| **pthread** | POSIX 线程；映射到 TCB |
| **signal trampoline** | libc 中恢复上下文并调用信号处理器的代码 |
| **futex** | 用户态快速锁；共享 Frame + Notification |

## 4. 存储与文件系统

| 术语 | 定义 |
| --- | --- |
| **VFS** | 虚拟文件系统；挂载表、路径解析、文件句柄、页缓存 |
| **Mount** | 文件系统挂载点；绑定 FS Backend Endpoint |
| **Dentry** | 路径组件到 inode 的缓存 |
| **Inode** | 文件元数据与后端节点句柄 |
| **ext2** | POSIX 根文件系统后端 |
| **tmpfs** | 内存文件系统；`/tmp`、运行时文件 |
| **devfs** | `/dev` 设备节点后端 |
| **procfs** | `/proc` 进程信息后端 |
| **Page Cache** | 文件数据缓存；Frame 共享 |

## 5. 平台与工程

| 术语 | 定义 |
| --- | --- |
| **HAL** | 硬件抽象层 trait 集合 |
| **BootInfo** | 内存 map、内核镜像、帧缓冲、DTB/RSDP、服务器模块 |
| **BootModule** | bootloader 提供的服务器原生镜像或根镜像 |
| **Hosted** | 在普通进程中运行的确定性模拟器 |
| **Conformance** | 所有平台通过的测试套件 |
| **Deterministic Sim** | 固定种子、虚拟时钟、可回放模拟 |
| **Native UI** | 可选原生真彩色显示体验；HolyDoc + Display Server |

## 6. 命名约定

- 内核类型：`Tcb`、`VSpace`、`HandleTable`、`Handle`、`Endpoint`、`Notification`、`Frame`、`Untyped`。
- 服务器：`hk-root-server`、`hk-vfs`、`hk-posix-server`、`hk-network`、`hk-holyfs`、`hk-display`、`hk-holydoc`。
- libc：musl 为主；`hk-abi`/`hk-crt` 提供 shim 与启动；`hk-libc` 薄兼容层；C ABI 符号使用标准 POSIX 名称。
- 路径：POSIX 风格 `/usr/bin/sh`；根文件系统由 ext2/ext4 Server 提供；HolyFS 提供 Holy 卷与 `H:` 盘符。
- 句柄：`Handle`、`Rights`、`HandleTable`；Endpoint badge 仅用于客户端标识。

## 7. 相关文档

- [章程](charter.md)
- [总体架构](architecture.md)
- [执行模型](execution.md)
- [服务器](servers.md)
- [VFS](vfs.md)
- [POSIX 与 libc](posix.md)
