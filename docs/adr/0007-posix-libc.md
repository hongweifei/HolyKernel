# ADR-0007：musl 移植 + 薄 hk-libc

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[POSIX 与 libc](../posix.md)、[服务器](../servers.md)

## 背景

应用需要标准 C 库与 POSIX 接口。完整自研 libc 覆盖面大、测试成本高；直接使用 glibc 需要大量 Linux syscall 假设。musl 体积小、代码清晰、易于移植，适合作为 POSIX 兼容环境的主 C libc。

## 决策

1. **musl 作为兼容环境的主 C libc**：提供 POSIX 函数、stdio、malloc、pthreads、locale、动态链接。
2. **`hk-abi`**：稳定的 POSIX syscall shim 协议与 C 头文件；把 musl 的 syscall 入口转换为 POSIX Server IPC。
3. **`hk-crt`**：启动代码、TLS 初始化、signal trampoline、ELF 加载与动态链接 shim。
4. **`hk-libc`**：薄兼容层；提供 HK 扩展、ABI 胶水与少量兼容符号，复用 musl 的完整实现。
5. musl 的 `ld-musl-*.so.1` 移植版作为动态链接器；支持静态与动态链接。
6. 应用使用标准 POSIX 头文件；针对 musl + `hk-abi` 重新编译。
7. 1.0 聚焦源码兼容；Linux 二进制 ABI 属于后续版本。

## 后果

**收益**

- 复用 musl 的成熟实现，减少自研 libc 的覆盖面与测试成本。
- POSIX 接口与微内核 IPC 通过 `hk-abi` 对齐；错误与权限模型一致。
- C/C++/Rust 应用共用 POSIX ABI；musl 生态工具链可直接使用。
- `hk-libc` 保持薄层，维护面小。

**成本与取舍**

- 需要维护 musl 的 HolyKernel 移植补丁与 syscall shim。
- 动态链接、TLS、signal trampoline 需要与 `hk-crt` 协调。
- 每次 POSIX 调用有 IPC 成本；需要快速路径与缓存。
- glibc 特有接口与 Linux 扩展属于后续版本。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 自研完整 libc | 与微内核模型贴合；覆盖面与测试成本高。 |
| glibc 移植 | 生态最大；Linux syscall 假设多，移植成本高。 |
| musl 移植 + 薄 hk-libc | 复用成熟实现；需要 shim 与 CRT 适配。 |
| 自带 libc 方案 | 系统更小；C 应用自带 libc，生态碎片化。 |

## 架构一致性检查

- musl 是兼容环境的主 C libc；`hk-abi` 是唯一 syscall shim 边界。
- `hk-crt` 负责启动、TLS、signal trampoline 与 ELF 加载。
- POSIX Server 维护进程/fd/信号状态；musl 维护线程/TLS/stdio。
- ELF 加载通过 POSIX Server 与 VFS 映射 Frame。

## 相关

- [ADR-0003：POSIX personality](0003-posix-personality.md)
- [ADR-0006：VFS](0006-vfs.md)
