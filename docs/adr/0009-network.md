# ADR-0009：用户态网络服务

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[网络](../network.md)、[服务器](../servers.md)、[POSIX 与 libc](../posix.md)

## 背景

应用需要 TCP/IP、UDP、DNS、DHCP 与 POSIX socket。内核保持最小，网络协议栈、socket 状态、策略与驱动适合放在用户态。

## 决策

1. **Network Server** 运行在用户态；内核不包含 TCP/IP、socket、路由或 netfilter。
2. **驱动用户态**：virtio-net 优先；e1000/igb 后续；驱动持有 NIC DeviceGrant。
3. **协议栈**：IPv4 + IPv6 基础、ARP/NDP、ICMP/ICMPv6、UDP、TCP、DHCP 客户端、DNS 解析与缓存。
4. **Socket 对象**：原生应用通过 Socket Handle 访问；rights 为 READ/WRITE/ADMIN。
5. **POSIX 兼容**：`hk-libc` socket shim → POSIX Server → Network Server；fd 映射到 Socket Handle。
6. **策略**：raw socket、特权端口、`listen` 需要 ADMIN；每客户端 socket/连接/backlog 配额。
7. **安全**：Network Server 用户态隔离；无 ambient network authority；解析器 fuzz；TLS 在用户态库。
8. **配置**：默认 DHCP；原生 `T:/Network/Config.hd`；POSIX `/etc/resolv.conf`、`/etc/hosts` 通过 VFS。

## 后果

**收益**

- 内核保持最小，网络故障不影响内核。
- Network Server 可重启；驱动与协议栈隔离。
- POSIX 应用使用标准 socket API；原生应用使用 Socket Handle。
- 网络策略与配额集中在 Network Server。

**成本与取舍**

- 数据路径多一次 IPC/Frame 传递；需要零拷贝与批处理。
- Network Server 是较大用户态组件；需要 fuzz 与审计。
- 无 IOMMU 时驱动 DMA 仍受信任。
- TLS/证书、Wi-Fi、USB 网卡属于后续版本。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 内核内 TCP/IP | 性能直接；内核膨胀，故障影响大。 |
| 单用户态网络服务 + POSIX socket | 隔离好；IPC 成本。 |
| 独立 socket/protocol 服务器 | 组件更细；跨服务器复杂度高。 |
| 仅 AF_UNIX/loopback | 范围小；无法满足网络需求。 |

## 架构一致性检查

- 内核仍只提供 TCB、VSpace、HandleTable、Endpoint、Notification、Frame、Untyped、IRQHandler。
- Network Server 通过 Net Endpoint 访问 Network Driver；Network Driver 持有 DeviceGrant。Network Server 通过 Socket Handle 服务客户端。
- POSIX Server 维护 fd 表；fd 映射到 Socket Handle。
- 网络配置与审计通过 Endpoint；无全局网络后门。

## 相关

- [网络](../network.md)
- [ADR-0005：Handle 与 IPC](0005-handles.md)
- [ADR-0007：musl 与 hk-libc](0007-posix-libc.md)
