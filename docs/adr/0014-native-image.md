# ADR-0014：原生任务镜像

- 状态：**已接受**
- 日期：2026-11-05
- 关联：[平台 HAL](0004-platform-hal.md)、[服务器](../servers.md)、[Crate 分层](../crates.md)

## 背景

原生服务的运行期契约由 HKX 任务镜像承载；ELF 作为外部工具链与兼容环境的格式。HKX 记录 Native Task、能力、内存区域、符号与内容标识，装载器按记录直接建立运行时对象。

## 决策

1. 原生服务与原生应用使用 HKX 任务镜像：32 字节头（魔数 `HKXI`、版本、头长度、记录数、记录区 CRC32C、内容标识、镜像长度），后接类型化 TLV 记录。
2. 记录覆盖运行时契约：
   - `Task`：名称、角色、Cadence 默认契约（类别、权重、预算、周期、亲和性）、入口、用户栈大小、Arena 大小；
   - `Region`：虚拟地址、内存长度、访问标志与内容；
   - `Capability`：能力类别、权利与名称，装载时由 Root Server 授予 Handle；
   - `Symbol`：地址与名称，随镜像携带；
   - `Dependency`：服务名与最低版本。
3. `hk-image` 提供构建、解析与校验；`xtask image` 在打包时把服务 ELF 转换为 HKX，ISO 内的服务模块以 `.hkx` 结尾且只包含 HKX。ELF 只作为编译器的中间产物。
4. Root Server 只解析 HKX：Task 决定入口、栈与 Arena；Capability 决定设备授权；Symbol 在装载时注册到 Debug Server；移除运行期 `xmas-elf` 依赖。
5. 内核镜像仍以 ELF 交给 Limine；这是引导器协议边界，不属于原生运行期格式。
6. POSIX/Linux 兼容层继续处理 ELF，因为那是外部应用格式；构建期符号提取也仍读取 ELF 构建产物。

## 后果

- 收益：镜像承载 Native Task 清单；服务布局、能力、符号与内容标识随镜像传递；运行期由原生镜像装载器解析；记录区可整体校验。
- 成本：打包多一步转换；装载时校验记录区；引导器边界仍要求 ELF。
- 中性：兼容环境继续支持 ELF。

## 备选方案

- 继续使用 ELF：与“原生格式自有”的目标冲突。
- 只替换 ELF 段表：仍是装载描述，未体现 Holy 运行时契约。
- 自写引导器接收扁平镜像：范围与风险显著扩大，留待后续。
- PE/COFF：同样属于外部格式，收益更小。

## 架构一致性检查

HKX 记录对应既有运行时对象：Task、Cadence、Meter、Arena、Handle、Endpoint、Notification、Frame。Root Server 仍通过 `UserMemory` 映射 Region；服务发现、监督与重启策略不变。

## 相关

- [ADR-0004：平台 HAL](0004-platform-hal.md)
- [ADR-0017：Holy Cadence Model](0017-holy-cadence.md)
- [服务器](../servers.md)
- [Crate 分层](../crates.md)
