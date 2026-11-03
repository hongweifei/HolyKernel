# ADR-0010：HolyFS 文件系统

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[HolyFS](../holyfs.md)、[服务器](../servers.md)、[VFS](../vfs.md)、[POSIX 与 libc](../posix.md)

## 背景

HolyFS 是 HolyKernel 的原生文件系统，覆盖整文档、盘符、压缩、快照。它可作为 POSIX `/` 主根，同时提供原生 Document API。HolyFS 与 ext4 都是一等候选。

## 决策

1. HolyFS 是**用户态一等文件系统后端**；实现 VFS Backend，可挂载为 `/`、`/home` 或其他挂载点。
2. 原生命名空间通过 `H:` 盘符访问同一 Volume；`H:` 与 VFS 挂载点指向同一份数据。
3. 同一套磁盘格式提供两个视图：**POSIX inode 视图**与**原生 Document API**。
4. POSIX 视图持久化 `mode`/`uid`/`gid`/`nlink`/时间戳；支持硬链接、符号链接、稀疏文件、原子 `rename`、`fsync`。
5. 内部采用 **COW + segment log + 双超级块 + checksum**；整文档写入原子提交。
6. 文件数据按 **extent** 存储：小文件连续，大文件多 extent；支持透明压缩。
7. 提供 **快照/回滚/GC**；`check`/`repair`/`scrub`。
8. 性能优化：页缓存、zero-copy Frame、group commit、多队列、每 inode 锁、并行压缩。
9. 由服务清单选择根文件系统：HolyFS 或 ext4/ext2；三者共用 VFS/POSIX Server。
10. `/dev`、`/proc`、`/tmp` 由 devfs/procfs/tmpfs 单独挂载；设备节点由 devfs 提供。
11. POSIX 权限由 VFS 与 HolyFS 共同检查；原生 Document API 通过 Handle，并采用与 POSIX 路径一致的权限规则。

## 后果

**收益**

- HolyFS 可作为主根，满足 POSIX 应用与原生文档两套需求。
- 同一份数据既可通过 VFS 访问，也可通过整文档 API 访问。
- COW + 双超级块 + checksum 提供崩溃一致性与完整性。
- Extent + cache + group commit 提供高吞吐。
- 快照/回滚适合系统恢复、文档版本与构建缓存。

**成本与取舍**

- 需要实现 POSIX inode、权限、硬链接、符号链接、稀疏文件，复杂度提升，覆盖 POSIX 主根能力。
- COW 写放大与 GC 复杂度。
- checksum 与压缩增加 CPU/空间开销。
- 主根故障影响面大；需要恢复流程与严格测试。
- 与 ext4 并存意味着两套文件系统后端都要维护。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 文档存储服务 | 实现简单，覆盖整文档与快照场景；POSIX 主根由 VFS 后端与 POSIX 元数据承担。 |
| VFS 后端 + POSIX 元数据 | 覆盖主根与文件语义；权限、硬链接、符号链接完整。 |
| 仅 ext4 主根 | 成熟；缺少原生整文档/压缩/快照。 |
| 内核内 HolyFS | 性能直接；破坏微内核边界。 |
| HolyFS 主根 + ext4 可选 | 统一原生体验；两套后端维护成本。 |

## 架构一致性检查

- HolyFS 是用户态服务，通过 Block Endpoint 访问存储。
- 文件系统运行在用户态；内核提供机制。
- 通过 VFS Backend 接口服务 POSIX Server；通过 Document Handle 服务原生应用。
- 可作为 `/` 主根；`/dev`、`/proc`、`/tmp` 仍由独立后端挂载。
- POSIX 权限与 Handle 权限同时生效；访问通过显式 Handle，权限来自 Handle。

## 相关

- [HolyFS](../holyfs.md)
- [ADR-0006：VFS](0006-vfs.md)
- [ADR-0007：musl 与 hk-libc](0007-posix-libc.md)
- [ADR-0009：网络](0009-network.md)
