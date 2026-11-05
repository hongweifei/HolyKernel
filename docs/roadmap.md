# 路线图

状态：**设计中** · 上级：[总体架构](architecture.md)

本文定义 M0–M11 里程碑、退出标准、决策门与风险。节奏由退出标准决定，以可演示的纵向切片推进。

---

## 1. 排序原则

1. **先 Hosted，后裸机**：内核与 IPC 在确定性模拟器中先跑通。
2. **先内核机制，后服务器**：TCB、VSpace、HandleTable、Endpoint、Notification、Frame 优先。
3. **先 Root Server，后全栈**：Root Server、Device Manager、VFS、Network、Display、POSIX 依次接入。
4. **VFS 先行**：ext2/tmpfs/devfs/procfs 通过 VFS 统一；POSIX Server 依赖 VFS。
5. **原生能力先行**：网络与原生 UI 在 POSIX 之前；POSIX 兼容层复用 Native Task、VFS、Network 与 Console。
6. **POSIX 分层**：先文件/进程，后线程/信号/POSIX socket。
7. **可选服务后置**：HolyFS 卷、Workbench、工具链等深化服务在核心链路之后接入。
8. **多架构收尾**：x86_64 纵向切片完成后抽 HAL，验证 AArch64/RISC-V。

---

## 2. 里程碑总览

| 里程碑 | 主题         | 关键产物                                               | 决策门             |
| --- | ---------- | -------------------------------------------------- | --------------- |
| M0  | 章程、架构、脚手架  | 文档、workspace、CI、xtask、Hosted stub                  | G0：架构批准         |
| M1  | Hosted 微内核 | TCB、VSpace、HandleTable、Endpoint、Notification、Frame | G1：内核对象冻结       |
| M2  | x86_64 启动  | Limine、BootInfo、Root Server、IPC                    | —               |
| M3  | 句柄与内存      | Handle duplicate/close、COW、SMP、调度                  | G2：句柄模型冻结       |
| M4  | 用户态驱动与 VFS | DeviceGrant、virtio、VFS、ext2/tmpfs/devfs/procfs     | G3：VFS 接口冻结     |
| M5  | 网络         | virtio-net、Network Server、IPv4/IPv6、TCP/UDP、DHCP/DNS | —               |
| M6  | 原生 UI      | Display Server、HolyDoc、Console、输入、字体/主题          | —               |
| M7  | POSIX 核心   | POSIX Server、musl、hk-abi/hk-crt、fork/exec/wait、文件  | G4：POSIX ABI 冻结 |
| M8  | POSIX 扩展   | pthreads、信号、pipe、AF_UNIX、mmap、POSIX socket        | —               |
| M9  | Linux ABI 兼容 | personality、Linux ABI Server、ELF、/proc、vDSO、兼容测试 | G5：Linux ABI 冻结 |
| M10 | 多架构与测试     | AArch64/RISC-V、POSIX/Linux ABI 套件、性能                | G6：平台抽象冻结       |
| M11 | 1.0        | 硬化、fuzz、发布镜像、真机                                    | G7：1.0          |

```mermaid
flowchart LR
    M0["M0 章程与脚手架"] --> M1["M1 Hosted 微内核"]
    M1 --> M2["M2 x86_64 启动"]
    M2 --> M3["M3 句柄与内存"]
    M3 --> M4["M4 驱动与 VFS"]
    M4 --> M5["M5 网络"]
    M5 --> M6["M6 原生 UI"]
    M6 --> M7["M7 POSIX 核心"]
    M7 --> M8["M8 POSIX 扩展"]
    M8 --> M9["M9 Linux ABI 兼容"]
    M9 --> M10["M10 多架构与测试"]
    M10 --> M11["M11 1.0"]
```

---

## 3. M0 — 章程、架构、脚手架

### 交付

- 章程、总体架构、内存、执行、服务器、VFS、POSIX、UI、平台、工程、路线图、术语、ADR。
- Cargo workspace；全部目标可编译（允许空实现）。
- `xtask`：`build`、`run`、`test`、`lint`、`docs`、`budget`。
- CI：`fmt`、`clippy`、`build-matrix`、`host-test`、`docs`、`deny`、`budget`。
- `hk-platform-hosted` stub：BootInfo、初始化、退出协议。
- 许可证确认。

### 退出标准

1. Hosted、x86_64、AArch64 三个目标构建成功。
2. `cargo xtask lint` 与 CI 全绿。
3. 架构文档评审通过；ADR-0001 至 0008 已接受。

---

## 4. M1 — Hosted 微内核

### 交付

- `hk-hal`：CPU、MMU、IRQ、Timer、SMP、Console、DMA。
- `hk-kernel`：TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Untyped。
- `hk-ipc`：send/recv/call/reply、Notification、句柄传递。
- `hk-handle`：HandleTable、duplicate/close、IPC 转移。
- Hosted 模拟器：虚拟 CPU、虚拟时钟、虚拟 VSpace。
- Root Server stub：加载服务器、分配句柄、IPC 回环。
- Native Task：VSpace+HandleTable+TCB(s)+Arena 的用户态封装；窗口由 Display Server 管理。

### 退出标准

1. Hosted 启动 Root Server，两个服务器通过 Endpoint 完成 call/reply。
2. Handle duplicate/close 与 IPC 转移测试通过。
3. VSpace 映射/解除映射/COW 单元测试通过。
4. 确定性模拟：固定种子回放一致。
5. 内核 panic dump 可复现。

---

## 5. M2 — x86_64 启动

### 交付

- `platforms/x86_64-qemu`：链接脚本、启动汇编、Limine、ISO。
- BootInfo 适配、早期串口、GDT/IDT、8254 PIT 定时器、帧缓冲文本。
- Root Server ELF 加载、初始 Handle 分配。
- 用户态服务器 IPC 回环；Device Manager stub。
- QEMU smoke 框架。
- Native Task 视图：任务名、Arena、子 Task、窗口列表。
- HolyShell 基础：`tasks`、`services`、`mounts`、`help`。
- HolyShell Session：Native Task、Session Namespace、`use`/`load`。

### 退出标准

1. QEMU 启动内核与 Root Server。
2. Root Server 启动两个用户态服务并完成 IPC。
3. 帧缓冲与串口输出正常。
4. 故意触发内核 panic/页错误，dump 完整。
5. 启动时间预算记录。

---

## 6. M3 — 句柄与内存

### 交付

- HandleTable、duplicate/close、IPC 转移。
- Untyped retype、Frame 共享、VSpace 映射、COW、Fault Endpoint。
- SMP 启动、per-CPU、IPI、抢占调度、tickless。
- 用户态线程创建/退出、通知与等待。
- HandleTable 审计与 trace。

已完成：服务退出关闭对端端点句柄、`restart` 重新装载已退出服务并统计重启次数、IPC 句柄转移（`SYS_SEND_HANDLE` 与接收句柄映射）、Notification 置位与阻塞等待（`SYS_NOTIFY`/`SYS_WAIT_NOTIFY`）、用户态 call/reply（`SYS_CALL`/`SYS_REPLY`）、Hosted COW fork 原型（父页共享只读、子页写时复制）、QEMU 每任务页表与 CR3 切换、用户态异常隔离（`hk-server-fault` 按需启动验证）、QEMU COW fork（`SYS_FORK` 复制页表、写保护页错误恢复、`hk-server-fork` 验证父子栈页独立）、HandleTable 审计（授予/关闭/转移/释放事件与 `trace` 命令）、上下文切换与 IPC 时延统计（`stats` 命令、`SYS_TSC`/`SYS_TSC_FREQ`、`hk-server-latency` 测量）、MADT 本地 APIC 枚举（`-smp 4` 冒烟）、Limine MP 启动 AP、per-CPU TSS/GDT/内核栈、per-CPU 调度与 AP 用户任务执行、AP 本地 APIC 定时抢占、TLB shootdown（代际邮箱、跨核 IPI 确认、COW fork 整表同步）、AP 空闲关闭 tick 并用唤醒 IPI 恢复调度。

### 退出标准

1. 2/4 核 SMP 启动；跨核 IPC、空闲 IPI 唤醒与 TLB shootdown 正常。
2. COW fork 原型在 Hosted 与 QEMU 通过。
3. Handle close 后旧引用失效。
4. 上下文切换 ≤ 1 µs；IPC call/reply ≤ 2 µs（QEMU 记录）。
5. 服务器崩溃后 Root Server 可关闭句柄并重启 stub。

---

## 7. M4 — 用户态驱动与 VFS

### 交付

- Device Manager：PCI/DTB 枚举、DeviceGrant、驱动启动。
- virtio-blk/input/net 驱动；IRQ → Notification。
- VFS Server：挂载表、路径解析、dentry/inode 缓存、FileHandle、页缓存。
- ext2、tmpfs、devfs、procfs 后端。
- Block Driver 与 VFS 的 Endpoint 接口。

已完成：`hk-block` 块契约与 RAM disk Block Server；独立 `hk-server-virtio` 驱动服务（DeviceGrant 移交、virtio-drivers 探测、DMA 页复用、块协议）；`hk-fs-ext4` 以 `rsext4` 实现 ext4 后端；VFS 通过驱动端点挂载 `/disk`；demo 经 VFS 写入并读回标记文件、用 `OP_SYNC` 刷写；宿主侧重新挂载镜像验证落盘；`SYS_RECV_DATA_FROM` 按发送者过滤，避免服务同时作为客户端时误收请求；IPC 单消息上限提升到 8 KiB，支持整块（4 KiB）读写；平台 RTC 在启动时建立民用时间锚点，由单调 tick 推进并通过 `SYS_CIVIL_TIME` 提供，ext4 后端在磁盘边界换算时间戳；Root Server 增加重启策略与监督循环，Device Manager 常驻、驱动按需申请授权；启动参数可配置 VFS 的 ext4 挂载点；demo 输出 1 MiB ext4 顺序读吞吐。内核与服务堆改为可回收分配器；IPC 单消息上限提升到 64 KiB，VFS 大块顺序读写绕过逐页路径，块与 VFS 客户端缓冲移到堆上；HolyFS v1 完成对象化重构（Document/Revision/Entry、文档与集合项共用修订链、Document/Revision/Collection 三棵 COW 元数据树、子集合、追加快路径与整文档读取）并通过 `vfs.fs=holyfs` 挂载与 QEMU `holyfs` 冒烟；共享 Frame 数据路径落地（`SYS_FRAME_*`、`SYS_HANDLE_DUPLICATE`、`SYS_RECV_DATA_HANDLE`，块设备与 VFS 的 attach 协议），驱动把帧物理地址直接交给 virtio DMA，VFS 文件读写直接落在客户端共享帧；客户端 detach 清理帧映射与会话句柄，Frame 最后句柄释放时归还 DMA 池；HolyFS inode 缓存与块客户端直写帧使文档路径零拷贝。
- VFS 重启与后端恢复。

### 退出标准

1. 挂载 ext4 数据盘（`rsext4`），读写文件，重新挂载后数据保留；支持通过启动参数把 ext4 作为 `/` 主根。
2. tmpfs/devfs/procfs 挂载与访问正常。
3. VFS 崩溃后重启，在途请求返回 `EIO`，新请求恢复。
4. 驱动退出后 Root Server 按重启策略重启，Device Manager 常驻并重新下发 DeviceGrant。
5. VFS 1 MiB 顺序读 ≥ 100 MiB/s；共享 Frame 与 HolyFS inode 缓存落地后 QEMU release 测得 ext4 约 81–104 MiB/s、ext4 主根约 98–108 MiB/s、HolyFS 约 155–195 MiB/s（debug HolyFS 约 25 MiB/s）；已接入 virtio-net 设备类、独立驱动服务与 `hk-server-net`，`smoke --mode net` 校验 MAC、Net Endpoint 握手、DHCP 租约、`ping 10.0.2.2`、TCP 回显与 UDP 回显；`hk-net` TCP/DNS API 已有双栈主机测试（含 A/AAAA/失败/超时），QEMU `net` 模式已连接宿主 echo 验证 TCP 握手/回显，并用 `net.dns=localhost` 验证 DNS 解析；内核 `send` 改为异步入队避免双向发送死锁，网络驱动改为非阻塞收发去掉 TX 自旋；Device Manager 缓存授权避免运行期改写 BAR 干扰在用设备，DNS 已纳入默认 `net` 冒烟；`net` 命令经用户态 `hk-server-netstat` 查询接口、收发统计、路由、ARP 邻居与原生配置（`hk-shell` 保持核心命令集，扩展点转交服务）；virtio-net 按 `Always` 重启，Network Server 检测端点变化并重新握手恢复链路（`smoke --mode netrestart`）；共享 Frame 收发环落地（驱动分配并登记 DMA 帧，RX/TX 零拷贝，`net.zerocopy` + `smoke --mode netzerocopy`）；原生 Socket Handle API 落地（`hk-server-api::socket` 客户端 + `hk-server-netsock` 用户态自检，`smoke --mode netsocket`）；ext4 已接近/周期性达到 100 MiB/s，ext4 仍受 rsext4 内部缓冲的一次拷贝限制。

---

## 8. M5 — 网络

### 交付

- virtio-net 驱动服务：复用 `virtio-drivers` `VirtIONetRaw`，DeviceGrant、RX/TX 队列、IRQ Notification、共享 Frame 收发环。
- Network Server：基于 `smoltcp` 的 Ethernet、ARP/NDP、IPv4/IPv6、ICMP、UDP、TCP、DHCP、DNS；自定义 PHY 适配 Net Endpoint。
- 原生 Socket Handle 与 Endpoint/Notification 收发；`net` 命令查看接口、ARP/路由、连接与统计。Socket API（TCP/UDP/DNS）经 `hk-server-api::socket` 提供给用户态服务。
- 静态地址与 DHCP 配置；支持启动参数 `net.dhcp=off`/`net.ip=`/`net.gw=`/`net.resolver=` 的 Holy 原生引导配置，并以 `smoke --mode netstatic` 验证；原生配置对象 `H:/Network/Config.hd`，经 Network Server Endpoint 读写；POSIX 兼容环境的 `/etc/network`、`/etc/hosts`、`/etc/resolv.conf` 由 POSIX 桥从原生配置投影，不是配置源头。
- 包路径使用共享 Frame，驱动与协议栈避免逐包复制；为后续 POSIX socket 复用同一 Socket Handle。

### 退出标准

1. QEMU 中 `net ping`、UDP echo 与 TCP 连接成功。
2. DHCP 获取租约；DNS 查询成功。
3. `net` 命令展示接口、ARP/路由、Socket 状态与收发统计。
4. 驱动退出后 Root Server 重启并恢复网络；在途请求返回明确错误。
5. 共享 Frame 收发环有单元测试与 QEMU 冒烟，吞吐与延迟记录到基准。

---

## 9. M6 — 原生 UI

### 交付

- Display Server：帧缓冲、窗口、合成器、输入焦点。已落地：帧缓冲仅授权 `hk-server-display`；窗口表面使用共享 Frame 经句柄转移，合成器绘制桌面、标题栏与窗口并回读校验；多窗口按槽位维护 z 序，提交、Tab 或底部任务栏按钮切换时提升并高亮焦点；标题栏与任务栏均用 8×16 位图字体渲染窗口标题；任务栏始终置顶，窗口创建与拖动限制在其上方；标题栏关闭按钮回收表面、句柄并通知客户端；右下角把手可缩放窗口，合成器按比例采样客户端表面；标题栏支持最小化与最大化/还原，任务栏可恢复最小化窗口；窗口带圆角与柔和投影，damage 区域按投影边距扩展；启动后记录全屏重绘最小值（QEMU 1280×800 实测约 3.0–3.3 ms）；失败退出由 Root Server 重启，Console 检测端点变化重建窗口；`ui.scale=2` 放大标题栏/任务栏字体与布局；深/浅调色板由 `theme=light` 选择，窗口/任务栏/光标统一取色；合成按 damage 区域重绘，键盘事件经 IPC 路由到焦点客户端，`smoke --mode display/pointer` 注入 Tab、拖拽、任务栏、缩放与关闭点击验证。
- Input Driver：用户态 virtio-input 驱动（DeviceGrant + IRQ 通知），把 evdev 键码解码为字节交给 Display 按焦点路由；支持 virtio-tablet 绝对指针与点击，Display 绘制光标、按 z 序命中窗口，并支持标题栏拖拽移动窗口；`smoke --mode inputdriver/pointer` 验证。
- Console Service：原生终端、字符与属性、真彩色 SGR；POSIX 终端模式在 M7 接入。已落地：内核非阻塞控制台读与输入通知，用户态行编辑/回显；有 Display 时 Console 创建自己的终端窗口，用 8×16 字体渲染并提交合成；支持增量 UTF-8 与中文双宽字形、光标与提示符；SGR 支持 16/256/真彩色、粗体/暗淡/下划线/反显，以及光标定位与行列擦除；键盘事件由 Display 按焦点转发给 Console 做行编辑，无 Display 时回退内核控制台。`smoke --mode console/terminal` 验证。
- HolyDoc Server：解析、布局、渲染、链接、`@exec`。已落地：HolyDoc 指令解析、自动换行布局、内置欢迎页与帮助页、链接跳转/历史、`@exec` 允许列表、窗口内点击坐标、滚动与关闭；启动落在欢迎页，经 `H:` 卷原生文档协议枚举集合并整篇读取文档，集合页与 DocPath 链接可直接跳转；`smoke --mode holydoc` 验证解析、执行、跳转、返回、集合浏览与渲染。
- sRGB 真彩色 + 主题、可缩放字体（8×16 位图回退）、窗口切换。已落地：深/浅语义调色板、`ui.scale` 缩放标题栏与任务栏；`hk-font` 内置 TrueType 字体，支持轮廓栅格化、字形缓存与缺字形 8×16 位图回退；Display 标题/任务栏与 Debug 窗口使用可缩放字体，Console 固定网格与 HolyDoc 布局继续使用位图回退。
- 原生欢迎页与文档浏览。已落地：欢迎页作为启动页，文档集按 `H:` 盘符枚举集合，文档索引与 DocPath 链接直接跳转。
- Debug Server：Task/资源视图、trace、`hk-dbg`、符号化栈回溯。已落地：调试窗口分页展示概览/任务/服务/trace/统计/异常，`hk-server-dbg` 客户端打印同样的视图；镜像构建生成内核与服务符号表，用户异常记录现场并经 Root 控制 `symbolize`/`fault` 输出符号化回溯；`smoke --mode debug` 验证各视图与符号解析。
- 服务器热重启与应用热替换原型。已落地：Display 热重启与 Console 重连；Root 控制 `replace <服务> <镜像>` 终止未在 CPU 上执行的任务，用另一原生模块启动同名服务，客户端按端点变化重连。`smoke --mode hotswap` 验证。
- Workbench：编辑/浏览/构建/运行/调试一体化。
- Toolchain Server：系统内构建、编译期执行、隐藏链接步骤。
- Checkpoint Server：Native Task 快照与恢复。
- HolyFS Core：对象表、修订链、连续 extent、checksum、双超级块、页缓存、VFS 适配层、整文档 API。已落地原生 Document API 与 `H:` 盘符：DocPath 解析、集合枚举、整文档读写、文档视图与修订链。
- Home/Once 启动文档：Shell 启动执行 `H:/Home`，`H:/Once` 首次执行后写 `Once.done`。已落地。
- Sprite/Font 编辑器。

### 退出标准

1. QEMU 启动到图形 Console 与 HolyShell。
2. HolyDoc 链接跳转、`@exec` 结果嵌入。
3. 多窗口、焦点、键盘/鼠标正常。
4. Display Server 崩溃后重启，Console 重连。
5. 帧缓冲全屏重绘 ≤ 8 ms。
6. HolyFS `H:` 盘符、文档读写、修订与快照、重启恢复。

---

## 10. M7 — POSIX 核心

### 交付

- POSIX Server：PID 表、fd 表、进程组、会话、cwd/root。
- musl 移植：C 运行时、stdio、malloc、pthreads；`hk-abi` 提供 syscall shim 与头文件；`hk-crt` 提供启动、TLS、signal trampoline、ELF 加载；`hk-libc` 薄兼容层。
- ELF 加载器与动态链接。
- `fork`/`exec`/`wait`/`exit`；文件 `open`/`read`/`write`/`stat`/`readdir`/`rename`/`unlink`。
- `/sbin/init`、`/bin/sh`、基础 coreutils；`posix` 兼容入口。
- 支持静态链接自有 libc（实现 `hk-abi` 后端）；musl 为一等支持，其他 libc 为 best-effort。
- Console Service 的 POSIX 终端模式：`/dev/console`、`/dev/tty`、行规程、`termios` 基础。
- POSIX fd 映射到 VFS FileHandle；网络 socket 在 M8 接入。

### 退出标准

1. QEMU 启动到 HolyShell；`posix sh` 可用。
2. `posix sh` 执行 `ls`、`cat`、`echo`、重定向与管道。
3. 静态与动态链接 C 程序运行；musl 作为兼容环境主 libc。
4. `fork` + `exec` `/bin/true` ≤ 5 ms。
5. POSIX 文件测试子集通过。

---

## 11. M8 — POSIX 扩展

### 交付

- pthreads：create/join/mutex/cond/rwlock/key/TLS。
- 信号：sigaction、kill、sigprocmask、trampoline。
- pipe、FIFO、AF_UNIX、共享内存、信号量。
- `mmap`/`mprotect`/`brk`。
- `select`/`poll`、`termios`、`isatty`。
- POSIX socket API：socket/bind/listen/accept/connect/send/recv/getaddrinfo，映射到 M5 的 Network Server Socket Handle。
- 动态 musl ABI 兼容（L1.5）：保持上游 musl ABI，提供 Holy `ld-musl`/`libc.so`；未修改的动态 musl ELF 直接运行。
- POSIX 测试套件扩展到进程/线程/信号/IPC/网络。

### 退出标准

1. 多线程 C/C++ 程序运行。
2. 信号 handler 与 `sigreturn` 正常。
3. pipe/socket 测试通过；`select`/`poll` 正常。
4. `mmap` 文件映射与共享内存测试通过。
5. POSIX `ping`、TCP 连接、DNS 查询、DHCP 租约通过。
6. 上游 musl 动态 hello、BusyBox、Alpine `sh`/`ls`/`cat` 无需重新编译运行；pthread/TLS/信号/`dlopen`/`getaddrinfo`/`termios` ABI 测试通过。
7. POSIX 测试子集通过率达到里程碑目标。

---

## 12. M9 — Linux ABI 兼容

### 交付

- 每任务 ABI personality：Native/POSIX 走 `int 0x80`，Linux 走 `syscall`；内核只转发原始 syscall。
- Linux ABI Server：ELF/`PT_INTERP`、auxv、TLS/FS/GS、信号帧、`clone`/futex、`mmap`、`epoll`/`ioctl`、socket、`/proc`/`/sys` 最小视图与 vDSO。
- x86_64 静态 musl/glibc 二进制 → 动态 glibc 二进制；BusyBox、coreutils 与语言运行时按阶段接入。
- `hk-compat` 兼容测试矩阵与 syscall 覆盖表；`hk-port` sysroot/工具链与 `holybuild` 打包。
- 兼容层安全边界：独立 VSpace/Native Task、Handle 权限、最小 `/proc` 视图与 fuzz。

### 退出标准

1. 静态 musl/glibc BusyBox 与基础 coreutils 直接运行。
2. 动态 glibc `ls`/`cat`/`sh` 运行，动态加载器与 `/etc/ld.so.cache` 正常。
3. `/proc/self/maps`、`/proc/self/stat`、auxv、vDSO 与 TLS 语义正确。
4. `hk-compat` P1/P2 syscall 覆盖与真实应用回归通过；ABI Server 重启可恢复。
5. 源码兼容（L1）目标应用零修改重编译；静态自带 libc 走 `hk-abi` 后端。

---

## 13. M10 — 多架构与测试

### 交付

- `hk-platform-aarch64`、`hk-platform-riscv64` 与 QEMU 启动目录。
- AArch64/RISC-V 的 Root Server、VFS、POSIX、Network、Linux ABI、HolyFS 全栈。
- POSIX/Linux ABI 测试套件扩展；VFS/ELF/驱动/网络/HolyFS fuzz。
- 性能基准与预算复核。
- 服务器重启矩阵（含 Network Server、HolyFS）。
- HolyFS COW 事务、GC、check/repair、scrub、性能优化与 `H:` 盘符启动。

### 退出标准

1. 4 个目标通过同一套 `boot/ipc/memory/irq/vfs/net/holyfs/posix/linux-abi/restart` 测试。
2. AArch64/RISC-V 上 HolyShell 与 `posix` coreutils 运行。
3. POSIX 测试子集在 x86_64 与 AArch64 通过。
4. 服务器崩溃/重启矩阵通过。
5. QEMU 接入 HolyFS `H:` 卷并运行文档测试。
6. 性能预算达标或记录例外 ADR。

---

## 14. M11 — 1.0

### 交付

- 硬化：fuzz 语料库、回归、长时间压力、故障注入矩阵。
- 文档：用户手册、libc 参考、VFS 指南、移植指南、故障排查。
- 发布镜像：x86_64 ISO、AArch64/RISC-V 镜像、Hosted 模拟器。
- 主流 x86_64 UEFI 真机验证。
- 安全/可靠性审计：`unsafe`、依赖、句柄回收、服务器重启。

### 退出标准

1. 所有里程碑退出标准成立。
2. conformance 在 Tier 1 + Tier 2 全绿；Tier 3 至少一台真机启动到 HolyShell。
3. 72 小时 SMP + 服务器重启 + VFS 故障注入：零死锁、零数据损坏、零未处理 panic。
4. POSIX 与 Linux ABI 测试套件达到公开目标；应用回归通过。
5. 构建可复现。
6. 文档完整；发布镜像与校验和齐全。

---

## 15. 风险与应对

| 挑战 | 影响 | 应对 |
| --- | --- | --- |
| 微内核 IPC 性能 | POSIX 应用变慢 | 快速路径、共享 Frame、批处理、基准门禁 |
| POSIX Server 复杂 | 进度拖延 | 分层实现：文件/进程 → 线程/信号 → 网络 |
| VFS 与后端一致性 | 数据损坏 | 单元 + 故障注入 + 后端重启测试 |
| `fork`/COW 语义 | 兼容性问题 | 早期原型、POSIX 测试、页错误跟踪 |
| 服务器重启状态恢复 | 应用中断 | 明确 EIO 语义、重连协议、会话恢复策略 |
| 多架构移植 | 范围扩大 | Tier 分层、x86_64 纵向切片优先 |
| libc 兼容范围 | 应用覆盖需要提升 | POSIX 测试套件 + 应用回归 + 增量接口 |
| 内核可信基 | 安全风险 | 最小内核、句柄模型、`unsafe` 审计 |

---

## 16. 当前状态

- 已完成：M0 脚手架（workspace、`xtask`、CI、`hk-hal`、`hk-test`、Hosted 平台与退出协议）、M1 Hosted 微内核（句柄表、Untyped/Frame/VSpace 与 COW、Endpoint/Notification、TCB 与调度、Root Server stub、确定性仿真）、M2 启动路径（Limine、BootInfo 适配与校验、COM1 串口、GDT/TSS/IDT、异常现场输出、内核分页与 MMIO 映射、本地 APIC 定时器、帧缓冲文本控制台、服务 ELF 加载、ring 3 用户态与 `int 0x80` 系统调用、服务表与 HolyShell 基础命令、多服务加载与内核消息 IPC、协作式任务切换、ACPI MADT 与 IOAPIC、PS/2 键盘中断、交互式 HolyShell、`tasks` 任务视图、TSC 启动耗时记录、用户态 Device Manager stub（PCI 枚举）、常驻 echo 服务与 HolyShell 会话 `focus`、按需 `start` 服务、APIC 定时抢占调度（10 ms tick）与 spin 验证、任务 Arena（`SYS_ALLOC`）与任务视图 Arena 列、Native Task 父子关系、Handle 计数与 `handles` 命令、句柄关闭与计数系统调用、服务重启计数、用户态运行时与系统调用分派下沉到 `hk-root-server`（运行时宿主）、Shell 执行循环与 I/O 宿主契约下沉到 `hk-shell`、启动模块 `.hsh` 脚本与 `use`/`load` 会话求值、服务退出关闭对端句柄与 `restart` 重新装载、IPC 句柄转移与接收句柄映射、Notification 置位与等待（`SYS_NOTIFY`/`SYS_WAIT_NOTIFY`）、用户态 call/reply（`SYS_CALL`/`SYS_REPLY`）、Hosted COW fork 原型、QEMU 每任务页表与 CR3 切换（任务视图显示 VSpace）、用户态异常隔离（页错误只终止故障任务）、QEMU COW fork（`SYS_FORK` 与 `hk-server-fork` 验证父子页独立）、HandleTable 审计事件与 `trace` 命令、上下文切换与 IPC call/reply 时延统计（`stats` 与 `hk-server-latency`）、MADT 本地 APIC 枚举与 `-smp 4` 冒烟、Limine MP 启动 AP、per-CPU TSS/GDT/内核栈、per-CPU 调度 current、AP 参与用户任务调度与跨核 IPC 唤醒、AP 本地 APIC 定时抢占、系统调用入口按架构分层、用户镜像装载下沉到 crates（`UserMemory`）、BootInfo 描述与 x86 引导初始化下沉到 crates、内核堆分配器下沉到 `hk-kernel`、服务镜像链接配置共享（`hk-server-build`）、QEMU 冒烟）。
- 当前代码：crate 分层与完整清单见 [Crate 分层](crates.md)；常驻服务在 `servers/`，自检客户端在 `clients/`，演示服务在 `demos/`，平台在 `platforms/`，构建与镜像工具在 `xtask/`。
- M2 收尾完成：HolyShell 会话（`use`/`load` 脚本求值）。
- 下一阶段：M5 网络已完成；**M6 原生 UI** 进行中（Display、Console、Input、HolyDoc 欢迎页与 `H:` 卷文档浏览）；POSIX 核心与扩展在 M7/M8，Linux ABI 兼容在 M9，多架构与 1.0 顺延到 M10/M11。

---

## 17. 相关文档

- [章程](charter.md)
- [总体架构](architecture.md)
- [服务器](servers.md)
- [VFS](vfs.md)
- [POSIX 与 libc](posix.md)
- [工程策略](engineering.md)
