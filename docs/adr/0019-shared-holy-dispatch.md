# ADR-0019：Holy 文档协议的共享分派

- 状态：已接受
- 日期：2026-11-18
- 关联：[ADR-0006](0006-vfs.md)、[ADR-0010](0010-holyfs.md)、[ADR-0016](0016-hmp-native-contract.md)

## 背景

Holy 文档协议同时由三类服务承载：

- VFS 与 Path：进程内后端（tmpfs/devfs/procfs）与远程 Holy 后端；
- ext4 独立服务：包装 `rsext4` 的 `Backend`；
- HolyFS 独立服务：原生 Holy 卷、修订链、快照、标签、配额与维护操作。

前两者共用 `hk-holy-service` 的 `DriveSource`/`Drives` 分派；HolyFS 曾维护一份
平行的 22 个 op 分派表（另外 31 个 HolyFS 专有 op）。两份表在文档读、写、
令牌与快照等语义上必须人工保持一致，属于典型的漂移点。

## 决策

1. 通用分派只依赖 `DriveSource` 契约与 `hk_fs::Backend`：
   - `DriveSource::list`/`volume` 提供盘符与卷视图；
   - `DriveSource::with_drive` 以回调形式把盘符临时视图暴露为
     `&mut dyn Backend` + 根文档。
2. `SHARED_OPS` 列出 22 个通用 op；具备原生后端的服务不得再自行实现。
3. HolyFS 以 `HolyDrive`（`Backend` 适配器）接入：
   - 读路径先查解压缓存，未命中再走 `read_range[_at]`；
   - 缓存通过 `RefCell<&mut ReadCache>` 注入，因为 `Backend::read` 是 `&self`。
4. `OP_ATTACH_DRIVE`/`OP_DETACH_DRIVE` 依赖 `ClientDisk`，留在服务入口的
   ServiceDisk 路径；`holy::handle` 只处理已挂载卷上的请求。
5. 共享 op 集合由测试覆盖：对每个 `SHARED_OPS` 发送最小合法请求，要求返回
   非 `STATUS_INVALID`。

## 后果

收益：

- 22 个 op 的请求解析、权限检查与响应编码只有一份；
- 新增后端只需实现 `Backend` 与 `DriveSource`，不必重抄分派；
- 协议语义变更在 `hk-holy-service` 一处生效。

成本与取舍：

- `RefCell` 引入运行时借用检查：禁止在缓存读取内部再次重入同一缓存；
- HolyFS 的 `handle`/`sync` 仍绑定 `ServiceDisk`（attach 依赖块客户端），
  宿主单测只能覆盖“已挂载卷 + 共享 op”，attach/detach 依赖冒烟；
- 分派表变为“共享 + 专有”两段，阅读时需要先看 `SHARED_OPS` 再读专有 op。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 维持两份分派表 | 零风险；协议演进时必然漂移，已出现 22 个 op 的重复实现 |
| HolyFS 把卷整体搬进 `Box<dyn Backend>` 注册表 | 分派完全统一；需重构卷所有权、缓存与 group commit，且丢失 HolyFS 专有类型 |
| 每次请求构造 `Drives` 适配器 | 无需长期持有后端；每请求分配与 `'static` 约束冲突 |
| 只写一致性测试、不共享代码 | 成本最低；测试只能覆盖可构造的等价状态，无法约束新增 op |

## 架构一致性检查

- 分派仍只经 Endpoint 与 `hk-proto` 线协议；服务之间不链接实现。
- 文档令牌与卷令牌仍由 `DocSessions`/`VolumeSessions` 管理，权利检查不变。
- `hk-fs-holyfs` 仍不依赖 `hk-vfs`；`hk-holy-service` 不依赖服务二进制。
- HolyFS 的修订、快照、标签与配额仍由其自身类型实现，不入共享层。

## 相关

- [Crate 分层](../crates.md)
- [服务器](../servers.md)
- [HolyFS](../holyfs.md)
- [VFS](../vfs.md)
