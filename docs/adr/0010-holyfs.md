# ADR-0010：HolyFS 文件系统

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[HolyFS](../holyfs.md)、[服务器](../servers.md)、[VFS](../vfs.md)、[POSIX 与 libc](../posix.md)

## 背景

HolyFS 是 HolyKernel 的原生文件系统。它以卷和盘符组织命名空间，以文档为内容单位，以修订记录历史，以集合表达原生关系。设计目标是现代、稳定、可靠、高性能。

Holy 语义围绕对象、关系与操作展开：身份是 DocId，名字是集合中的关系，内容是版本化的文档，访问通过带权利的句柄完成。POSIX 兼容环境通过 VFS 适配层访问同一批文档；适配层承担路径、偏移、fd 与凭证语义。

本 ADR 定义 HolyFS 的对象模型、原生关系、提交协议与操作；兼容语义由 VFS 适配层承担，HolyFS 磁盘格式保存 Holy 对象与关系。

## 决策

1. HolyFS 是**用户态 HolyKernel 原生文件系统**；以盘符（`H:` 等）与文档工作。
2. 对象模型包含 **Volume、Document、Revision、Entry、Tag、Snapshot、Handle、Drive**。
3. 集合是一份文档：文档 head 指向一条修订，修订 payload 承载内容、集合项索引或两者。内容与集合项使用同一条修订链。
4. 身份采用 **DocId（128 位）**；RevisionId 在卷内单调；DocPath 是查找视图，身份由 DocId 承载。
5. 原生关系为 **contains、references、revises、stores、pins、grants**；文档由集合项、标签、快照与封存修订保活。
6. 变更协议：每次变更携带 **expected**（文档 head、集合 head 或卷 generation）；`Create` 原子创建文档与集合项；`Bind`、`Unbind`、`Relocate` 生成新的集合修订。
7. 提交协议：**全部元数据以 COW 树存储**，Superblock 保存全部表根；写顺序为内容 → 修订 → 文档 → 集合修订 → 表根 → Superblock，数据、元数据与 Superblock 分阶段 flush；双超级块按 generation 选择最新有效代。
8. 修订保留：`Seal` 标记保留；`Prune` 保留最近 N 条未封存修订与全部封存修订并生成新基线；`Purge` 销毁文档；快照固定时返回 `Pinned`。
9. 垃圾回收：从当前根、全部快照、全部封存修订与标签表做**可达性标记**；读者以修订代与宽限期固定视图。
10. 空间管理：卷按**分配组**组织，组内维护位图与连续空闲段摘要；集合项使用**名字哈希 B-tree**；写入使用 `Stage`/`WriteRange`/`Commit` 流式协议，`Append` 共享基线 extent。
11. 句柄采用**服务端 128 位不可伪造令牌**，携带权利与卷代；`Grant` 收窄权利，`Revoke` 立即失效，`Restore` 使旧代令牌返回 `Stale`；盘符由**卷注册表**分配。
12. 兼容层：POSIX 兼容环境通过 **VFS 适配层**访问 HolyFS；适配层合成 `mode`/`uid`/`gid` 与时间字段。ext2/ext4 作为 POSIX 根文件系统；HolyFS 卷作为数据卷接入兼容环境的挂载点，并以盘符供原生应用访问。
13. 格式：小端、版本号、feature flags 与挂载校验；现代能力包括修订、快照、标签、封存、流式写、压缩、去重、配额与在线校验，按路线图分阶段落地。

## 后果

**收益**

- 磁盘格式与原生接口保持 Holy 模型：卷、文档、修订、集合、标签与快照一一对应。
- 内容与集合项共用修订链，快照完整固定集合视图与标签视图。
- 全元数据 COW 与 Superblock 根切换提供崩溃一致性与整文档原子提交。
- 身份与名字分离：文档可被多个集合引用，改名与移动保持身份。
- 分配组、名字索引与流式写提供可预测的顺序性能与大文档能力。
- POSIX 兼容环境通过适配层接入，兼容语义与 Holy 存储模型彼此独立。

**成本与取舍**

- 文档写入分配新 extent；旧修订按保留策略回收，需要 GC 与 scrub。
- 修订链、标签、快照与去重增加元数据与回收复杂度。
- 单用户模型把多用户凭证留给兼容层。
- 适配层维护路径、偏移、句柄与 POSIX 元数据映射。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| Holy 对象模型 | 卷、文档、修订、集合、标签、快照统一；原生语义完整。 |
| 文档存储 + 兼容视图 | 兼容路径短；磁盘格式承担两套语义。 |
| ext4 单一后端 | 兼容生态成熟；Holy 文档、盘符、修订与快照由 HolyFS 提供。 |
| 内核内 HolyFS | 路径短；文件系统策略进入内核，可信基扩大。 |

## 架构一致性检查

- HolyFS 运行在用户态，通过 Block Endpoint 访问存储。
- 内核提供 Block、Endpoint、Frame 与 Handle 机制。
- 原生应用通过 Document API 访问卷；POSIX 兼容环境通过 VFS 适配层访问同一份文档。
- 卷以盘符进入 Holy 命名空间；VFS 挂载点是兼容环境的视图。
- 快照、COW 与回收由文件系统服务维护；Root Server 负责服务生命周期与盘符注册。

## 相关

- [HolyFS](../holyfs.md)
- [ADR-0006：VFS](0006-vfs.md)
- [ADR-0007：musl 与 hk-libc](0007-posix-libc.md)
- [ADR-0009：网络](0009-network.md)
