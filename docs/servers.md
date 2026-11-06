# 用户态服务器

状态：**设计中** · 上级：[总体架构](architecture.md)

HolyKernel 的内核只提供机制；服务器承载策略。本文描述 Root Server、设备管理、驱动、VFS、文件系统、POSIX、网络与显示服务，以及服务监督与句柄分配。

---

## 1. 服务器全景

```mermaid
flowchart TB
    Root["Root Server<br/>监督与句柄分配"]
    DM["Device Manager<br/>PCI / DTB"]
    Blk["Block Driver<br/>virtio-blk / AHCI"]
    Net["Network Driver<br/>virtio-net"]
    Input["Input Driver<br/>virtio / PS2"]
    VFS["VFS Server<br/>DocId · 集合 · 修订 · 令牌"]
    Path["Path Server<br/>路径 · fd · 页缓存"]
    Ext2["ext2/ext4 Server"]
    Tmpfs["tmpfs Server"]
    Devfs["devfs Server"]
    Procfs["procfs Server"]
    POSIX["POSIX Server<br/>进程 · fd · 信号"]
    NetSrv["Network Server<br/>socket 后端"]
    HolyFS["HolyFS Server<br/>Holy 卷 · H: 盘符"]
    Display["Display Server<br/>真彩 UI"]
    HolyDoc["HolyDoc Server"]

    Root --> DM
    Root --> VFS
    Root --> POSIX
    Root --> NetSrv
    Root --> HolyFS
    Root --> Display
    DM --> Blk
    DM --> Net
    DM --> Input
    Path --> Ext2
    Path --> Tmpfs
    Path --> Devfs
    Path --> Procfs
    Path --> VFS
    VFS --> HolyFS
    HolyFS --> Blk
    POSIX --> Path
    POSIX --> NetSrv
    POSIX --> Root
    Display --> Input
    HolyDoc --> Display
    HolyDoc --> HolyFS
```

---

## 2. 服务清单

```rust
pub struct ServiceManifest {
    pub name: Name,
    pub elf: DocPath,             // 服务器镜像路径
    pub args: Vec<String>,
    pub handles: Vec<HandleSpec>, // 初始 Handle 需求
    pub endpoints: Vec<EndpointSpec>,
    pub depends_on: Vec<Name>,
    pub restart: RestartPolicy,   // Always | OnFailure | Never
}
```

- Root Server 读取服务清单，按依赖顺序启动服务器。
- 每个服务 Task 拥有独立的 VSpace、HandleTable、Endpoint 与最小 Handle 集合。
- 服务通过 Endpoint 暴露接口；客户端通过 Handle 获取服务入口。
- 服务清单由 bootloader 作为模块提供；运行时可从 VFS 加载更新版本。

---

## 3. Root Server

- 启动时获得初始 Untyped、HandleTable、Endpoint 与设备句柄。
- 解析服务清单，创建服务器 VSpace/TCB，按原生任务镜像的 Task 清单加载并分配 Handle；Capability 决定设备授权，Symbol 注册到 Debug Server。
- 维护服务表：PID、状态、重启次数、Endpoint、Handle。
- 维护每 Task/服务器的 Handle 配额、使用量与峰值；异常增长触发告警。
- 监督服务器：按 `RestartPolicy`（Never / OnFailure / Always）在退出后重启，关闭旧句柄并重新分配；`hk-server-device` 与 `hk-server-virtio` 默认为 Always。
- 监督循环按有界步进调度，避免持续 yield 的服务阻塞重启时机。
- 提供管理接口：`start`、`stop`、`restart`、`status`、`list`。
- 热替换：`replace <服务> <镜像>` 终止未在 CPU 上执行的任务，用另一原生镜像启动同名服务；服务表与设备授权保留，端点和句柄重新分配，客户端按端点变化重连。
- 检查点原型：`checkpoint <任务>` 暂停非当前任务并保存寄存器、Arena 用量与 VSpace，`restore-checkpoint <任务>` 在同一任务槽重建 TCB 并恢复原地址空间；持有设备或共享 Frame 的任务暂不支持，快照保存在 Root 内存。
- Root Server 自身崩溃时，内核按配置停机或重启。

---

## 4. Device Manager

- 枚举 PCI（ECAM）或 DTB 设备。
- 为每个设备创建 `DeviceGrant`：MMIO 范围、DMA 窗口、IRQ；virtio 设备的 grant 通过 IPC 移交给独立的 virtio 驱动服务。Device Manager 已生成 BAR/IRQ/capability 描述，通过 `SYS_DEVICE_GRANT_CREATE` 创建内核对象，并用 `SYS_DEVICE_MAP` 映射 MMIO、`SYS_DMA_ALLOC` 在授权 DMA 窗口内按页分配并映射，`SYS_DMA_SYNC` 维护缓存一致性、`SYS_IRQ_BIND` 绑定通知。
- 服务镜像通过 `.hk_service` 段声明角色，启动器据此生成 `ServiceSpec`；Device Manager 通过启动清单授予的设备授权工厂句柄创建 DeviceGrant；DMA 窗口在授权关闭或任务退出时归还，IRQ 路由按引用计数解绑。
- 按设备类别启动驱动：块、网络、输入、显示。独立 `hk-server-virtio` 驱动服务已接入 virtio-blk，完成容量查询、读、写与 flush 验证；Block Server 保留 RAM disk 后端与块协议。
- Device Manager 常驻并等待驱动的设备授权申请（`DEVICE_REQUEST`），驱动重启后重新枚举并下发 DeviceGrant。
- 维护设备树与驱动绑定；设备移除时关闭 Handle。
- 通过 Endpoint 向 VFS 与 POSIX Server报告设备状态。

---

## 5. 驱动

| 驱动 | 设备 | 接口 |
| --- | --- | --- |
| **Block Server** | RAM disk 与块协议服务 | Block Endpoint：read/write/flush/capacity |
| **virtio 驱动** | virtio 设备类别；当前接入块设备 | Block Endpoint：read/write/flush/capacity |
| **AHCI** | SATA 块设备 | Block Endpoint |
| **virtio-net** | 网络设备 | Net Endpoint：tx/rx |
| **virtio-input** | 键盘/鼠标 | Input Endpoint |
| **PS/2** | 键盘/鼠标 | Input Endpoint |
| **Framebuffer** | 显示 | Display Endpoint |

- 驱动运行在用户态；通过 DeviceGrant 映射 MMIO、分配 DMA Frame、绑定 IRQ。
- 中断通过 IRQHandler → Notification 交付驱动线程。
- 驱动崩溃后由 Device Manager 或 Root Server 重启；设备状态重新初始化。

---

## 6. VFS 与文件系统

- **VFS Server**：Holy 原生命名空间；盘符、DocId、集合、修订、文档令牌；不维护路径或 fd。
- **Path Server**：路径、目录、dentry、文件句柄与页缓存；为 POSIX 客户端提供 `/disk`、`/tmp`、`/dev`、`/proc` 投影，是原生 VFS 与路径后端之间的兼容层。
- **ext2/ext4 Server**：POSIX 根文件系统；目录、inode、权限位。
- **HolyFS Server**：HolyKernel 原生文件系统；`H:` 盘符与文档对象。
- **tmpfs Server**：内存文件系统；`/tmp`、运行时文件。
- **devfs Server**：`/dev` 设备节点；转发到设备 Endpoint。
- **procfs Server**：`/proc` 进程信息；从 POSIX Server 获取数据。
- VFS 与 HolyFS 通过 Holy 协议通信；Path Server 通过 Holy 客户端把 VFS 盘符投影为路径，再通过 `VFS_ATTACH`/共享 Frame 服务 POSIX 客户端。
- Native VFS 通过独立 virtio 驱动服务的 Block Endpoint 挂载 ext4（`rsext4`）为 E:，并注册 tmpfs/devfs/procfs 为 T:/D:/P:；Path Server 不直接访问块设备，也没有本地 tmpfs 挂载入口，只把这些盘符投影到 `/` 或 `/disk`、`/tmp`、`/dev`、`/proc`。HolyFS 由独立服务提供 H:/I:，Path Server 通过原生 VFS 的远程后端访问。

详见 [VFS](vfs.md)。

---

## 7. POSIX Server（兼容环境）

- 维护 PID 表、父子关系、进程组、会话、fd 表、信号状态。
- 处理 `fork`/`exec`/`wait`、`kill`、`sigaction`、`pthread_create` 等请求。
- 通过 VFS 执行文件操作；通过 Network Server 创建 socket。
- 通过内核 Handle 创建/操作 VSpace、TCB、Frame。
- POSIX Server 是可信基的一部分；崩溃后 Root Server 重启，POSIX 应用按策略恢复或终止。

详见 [POSIX 与 libc](posix.md)。

---

## 8. Network Server

- 管理网络设备与协议栈；支持 IPv4/IPv6、TCP/UDP、DHCP、DNS、loopback、AF_UNIX。
- 向 POSIX Server 提供 socket 后端：`socket`、`bind`、`connect`、`send`、`recv`、`poll`。
- 网络驱动通过 Net Endpoint 提供收发队列；数据使用共享 Frame。
- 共享 Frame 收发环：驱动分配并登记 DMA 帧，RX 直接 DMA 到共享帧、TX 由协议栈写入后零拷贝提交；`net.zerocopy` 启用。
- 原生 Socket Handle：`hk-server-api::socket` 客户端提供 open/connect/send/recv/close、UDP 收发与 DNS 解析；Network Server 处理请求；`hk-server-netsock` 用户态自检。
- 状态查询：`hk-server-netstat` 是独立用户态客户端，向 Network Server 发送 `NET_STATUS_REQUEST`/`NET_ARP_REQUEST`，打印 MAC、IPv4/前缀、网关、Socket 数、收发计数、路由视图、ARP 邻居与原生配置（DHCP/地址/网关/DNS/MTU）；`net` 命令在两种 shell 下语义一致。
- TLS/Wi-Fi/USB 网卡属于后续版本；详见 [网络](network.md)。

---

## 9. HolyFS Server

- 独立用户态服务 `hk-server-holyfs`；直接通过 Block Endpoint 挂载 Holy 卷，以 `H:`、`I:` 盘符提供原生文档命名空间。
- 原生 VFS 通过远程 Holy 后端连接该服务；Path Server 再把 VFS 的 `H:`、`I:` 投影到 `/disk`、`/disk-I`。VFS 不再内嵌 HolyFS。
- 对象模型：卷、文档、修订、集合、标签、快照；文档由集合项、标签与快照保活。
- 原生操作：`Stage`/`WriteRange`/`Commit`/`Read`/`Append`、`Bind`/`Unbind`/`Relocate`、`Checkout`/`Seal`/`Prune`、`Snapshot`/`Restore`/`Forget`、`Grant`/`Revoke`。
- 通过 Block Endpoint 访问存储；全元数据 COW、checksum、双超级块。原生客户端直接向 `hk-server-holyfs` 发送 Holy 协议请求，并可按块服务动态 `Attach`/`Detach` 盘符。
- POSIX 兼容环境的根文件系统为 ext4/ext2；HolyFS 作为 Holy 卷服务。
- 详见 [HolyFS](holyfs.md)。

---

## 10. Display Server 与 HolyDoc

- **Display Server**：帧缓冲、窗口、输入、sRGB 真彩色 + 主题、可缩放字体（8×16 位图回退）。
- **HolyDoc Server**：活文档渲染、链接、代码块、表单。
- **Workbench**：编辑、浏览、构建、运行、调试一体化应用。
- **Toolchain Server**：系统内编译/链接/打包/缓存。
- **Checkpoint Server**：Native Task/Server 快照与恢复。
- **Debug Server**：分页 Task/资源/trace 视图与最近异常符号化回溯；`hk-server-dbg` 客户端输出同样的视图。
- **HolyShell**：默认命令行；Holy 命令、结构化管道、脚本。
- **Console Service**：纯终端数据面，只做行编辑、回显、UTF-8/宽字符与按行交付；不解析命令。用户态 Shell 未启动时保留输入行，收到读行请求后再交付。
- **Shell 分层**：`hk-shell` 只提供命令解析、格式化与 `ShellHost`/`ShellIo` 契约，保持零依赖。内核态 shell 直接使用平台 runtime/loader，负责文本模式与恢复；用户态 `hk-server-shell` 通过 `ShellIo` 访问 Console，通过 Root Server 控制 ABI（`SYS_ROOT_CONTROL`/`SYS_ROOT_RESULT`）执行 `tasks`、`services`、`start`、`restart` 与脚本读取，`mounts` 作为用户态宿主扩展通过 VFS 只读查询挂载表，不进入 `hk-shell` 核心命令表，也不链接 `hk-runtime`/`hk-root-server`。两类 Shell 共享解析/格式化，但能力与权限边界独立；Console 不承担第三套命令。
- **POSIX Bridge**：`posix` 入口；连接 POSIX Server 与兼容工具。
- POSIX 应用通过终端/控制台接口使用文本界面；原生应用通过 Display Endpoint 使用窗口。
- 显示服务崩溃后由 Root Server 重启；窗口与输入状态重新建立。

详见 [HolyDoc 与 UI](ui.md)。

---

## 11. 服务重启与句柄回收

```mermaid
sequenceDiagram
    participant Root as Root Server
    participant Svc as Server
    participant K as Kernel
    Root->>K: 关闭旧 Handle / Endpoint
    Root->>Svc: 终止旧实例
    Root->>K: 创建新 VSpace / TCB
    Root->>Svc: 加载 HKX 镜像、分配 Handle
    Svc->>Root: 注册 Endpoint
    Root->>Root: 更新服务表
```

- 重启时关闭旧 Handle，防止旧实例继续访问资源。
- 客户端持有旧 Endpoint 时收到 `DeadEndpoint`；重新连接新实例。
- VFS、POSIX、Network 等有状态服务需要恢复策略：重建句柄、重新打开文件、重新绑定 socket。
- Root Server 记录重启次数与最后错误，供诊断。

---

## 12. 保证

1. 服务器运行在用户态，拥有独立 VSpace 与最小 Handle 集合。
2. 服务器通过 Endpoint 通信；客户端通过 Handle 获取入口。
3. Root Server 维护服务表与重启策略。
4. 驱动通过 DeviceGrant 访问 MMIO/DMA/IRQ。
5. VFS 是 POSIX 文件语义的统一入口；文件系统以服务器形式接入。
6. POSIX Server 维护进程、fd 与信号；VFS 维护挂载与文件句柄。
7. 服务器崩溃后 Root Server 关闭旧句柄，新实例重新注册 Endpoint。
8. 服务器清单与 Handle 需求可审计。
9. 每个服务器有 trace 与健康状态。
10. 服务器策略与文件系统逻辑位于用户态；内核提供机制。

---

## 13. 测试策略

| 层级 | 测试 |
| --- | --- |
| Hosted 单元 | 服务清单解析、Handle 分配、Endpoint 注册、重启状态机 |
| Hosted 模拟 | 服务器崩溃/重启、句柄回收、客户端重连、VFS/POSIX 恢复 |
| QEMU | Root Server 启动全栈、驱动 probe、VFS 挂载、HolyShell、`posix` |
| 故障注入 | 驱动崩溃、VFS 崩溃、POSIX 崩溃、Network 超时 |
| 基准 | 启动时间、服务器重启时间、IPC 延迟、VFS 吞吐 |

---

## 14. 相关文档

- [总体架构](architecture.md)
- [网络](network.md)
- [VFS](vfs.md)
- [HolyFS 文件系统](holyfs.md)
- [权限模型](permissions.md)
- [POSIX 与 libc](posix.md)
- [HolyDoc 与 UI](ui.md)
- [平台与启动](platforms.md)
- [工程策略](engineering.md)
