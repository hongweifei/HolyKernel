# ADR-0013：virtio-net 与 smoltcp 网络栈

- 状态：**已接受**
- 日期：2026-11-04
- 关联：[网络](../network.md)、[服务器](../servers.md)、[路线图](../roadmap.md)

## 背景

M5 需要 Ethernet、ARP/NDP、IPv4/IPv6、ICMP、UDP、TCP、DHCP 与 DNS。自行实现完整协议栈成本高、测试面大，且与驱动/服务边界无关。`virtio-drivers` 已提供 virtio-net 设备类；`smoltcp` 提供无堆、无 std 的 TCP/IP 协议栈，适合用户态 Network Server 的轮询模型。

## 决策

1. **virtio-net 驱动复用 `virtio-drivers` 0.13 `VirtIONetRaw`**：非阻塞 RX/TX、MAC、队列与 IRQ；不自行实现 virtio-net。
2. **协议栈复用 `smoltcp`**：启用 `medium-ethernet`、`proto-ipv4`、`proto-ipv6`、`socket-icmp`、`socket-udp`、`socket-tcp`、`socket-dhcpv4`、`socket-dns`，关闭 std/默认特性。
3. **驱动与协议栈分离**：独立 `hk-server-virtio-net` 持有 NIC DeviceGrant；`hk-server-net` 持有 smoltcp `Interface`/`SocketSet`，通过 Net Endpoint 与共享 Frame 收发以太网帧。
4. **smoltcp PHY 适配层**：Network Server 实现自定义 `phy::Device`，把本地 Frame 队列接到 smoltcp；`poll` 由 RX Notification/定时调度驱动。
5. **Socket Handle 自研**：smoltcp 只提供协议栈；Native Socket 表、rights、配额、Endpoint 协议与 `net` 命令由 `hk-net`/Network Server 提供。
6. **数据路径**：驱动侧 RX/TX 使用共享 Frame，保持零拷贝；smoltcp 边界允许一次复制，后续再做零拷贝 PHY。
7. **不选 `embassy-net`/`lwIP` 作为 M5 主路径**：前者引入 async executor，后者引入 C 构建依赖；需要时再评估。
8. **POSIX socket 后置**：M5 只提供原生 Socket Handle；M10 由 POSIX Server 映射 fd 到同一批 Socket Handle。

## 后果

**收益**

- 复用成熟 TCP/IP 与 virtio-net 实现，M5 聚焦服务边界、Frame 队列、权限与集成。
- smoltcp 无堆/无 std，适合裸机用户态服务；轮询模型与现有 Notification 调度一致。
- 驱动与协议栈独立重启；网络故障隔离在用户态。
- Socket Handle 与 POSIX 兼容层共享同一后端，避免两套连接状态。

**成本与取舍**

- smoltcp 位于 Network Server 内，需要自定义 PHY 适配与缓冲管理。
- 驱动与协议栈之间需要 Frame 队列、所有权与流控；初始版本允许 smoltcp 边界复制。
- smoltcp 的 DNS/DHCP 特性需锁定版本并做覆盖测试。
- 与 Linux socket 语义仍有差异；完整 POSIX 选项在 M10 增量补齐。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| smoltcp + virtio-drivers | 无 std、集成直接；需要 PHY/Frame 适配。 |
| embassy-net | async 抽象好；引入 executor 与更高集成复杂度。 |
| lwIP | 成熟、功能全；C 构建与许可/边界复杂。 |
| 自研协议栈 | 完全可控；测试面与工作量过大。 |

## 架构一致性检查

- 驱动持有 DeviceGrant，通过 IOMMU/DMA/IRQ 与设备交互；Network Server 只处理 Frame 与协议。
- smoltcp 运行在用户态 Network Server，不进入内核；内核提供 Task/Cadence、Endpoint/Notification/Frame/Handle。
- Socket Handle 由 Network Server 签发；POSIX Server 在 M10 做 fd 映射，不复制协议状态。
- 驱动与 Network Server 均可由 Root Server 重启；客户端按服务名重连。

## 相关

- [ADR-0009：用户态网络服务](0009-network.md)
- [网络](../network.md)
- [路线图](../roadmap.md)
