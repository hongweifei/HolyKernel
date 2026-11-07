# ADR-0012：musl 动态 ABI 兼容

- 状态：**已接受**
- 日期：2026-11-04
- 关联：[应用兼容](../compat.md)、[POSIX 与 libc](../posix.md)、[路线图](../roadmap.md)

## 背景

L1 源码兼容要求应用重新编译；L2 Linux ABI 覆盖未修改的 Linux ELF，但 glibc/静态二进制的 syscall 面很大。动态 musl 二进制是一个更划算的中间层：musl 没有 glibc 式符号版本，动态加载器小，公开 ABI 稳定；只要保持上游 musl 的 ABI，并替换 syscall 后端，就能让未修改的动态 musl 程序直接运行，而不需要 Linux syscall 转发。

## 决策

1. **新增 L1.5 兼容层**：未修改的动态 musl ELF 直接使用系统 `ld-musl-x86_64.so.1` 与 `libc.so`；syscall 由 Holy musl 后端翻译为 `hk-abi`。
2. **保持上游 musl ABI**：公开头文件布局、结构体、TLS、`pthread_*`、`FILE` 不透明句柄与无版本符号保持 musl 1.2.x 兼容；只替换 syscall/arch/CRT/loader 的 Holy 后端。
3. **加载器与 soname 兼容**：支持 `PT_INTERP=/lib/ld-musl-x86_64.so.1`、`libc.so` soname 与 Alpine 常见 `libc.musl-x86_64.so.1` 别名；提供 Linux 风格初始栈、auxv、`AT_*` 与 TLS。
4. **符号与动态加载**：保持 musl 无符号版本与动态加载器内部符号；`dlopen` 只保证 musl ABI 的 `.so`。
5. **直接 syscall 兼容**：应用通过 `syscall()` 传 Linux syscall 号时，做有限号段翻译；不支持的号返回 `ENOSYS` 并记录覆盖缺口。
6. **边界明确**：静态 musl 与 glibc 二进制仍走 L2；绕过 libc 直接发 Linux syscall 的程序不属于 L1.5。
7. **里程碑**：L1.5 在 M10（兼容层）交付并设 ABI 门禁；Linux ABI 与 glibc/静态二进制在同一里程碑推进。
8. **验收**：上游 musl 动态 hello、BusyBox、Alpine `sh`/`ls`/`cat`、pthread/TLS/信号/`dlopen`/`getaddrinfo`/`termios` 通过。

## 后果

**收益**

- 动态 musl 生态（Alpine 等）无需重新编译即可运行，是“无需移植”的最短路径。
- 复用同一份 musl 源码与 POSIX Server，不引入 Linux syscall 兼容面。
- 比 glibc ABI 兼容成本低：无符号版本、无 glibc 内部假设、加载器小。
- 与 L1 源码兼容共享 libc 与系统服务，测试面集中。

**成本与取舍**

- 必须严格冻结公开 ABI 与结构布局，任何 musl 升级都要跑 ABI 回归。
- Loader/auxv/TLS/信号帧仍需用户态实现与测试。
- 应用若直接发 Linux syscall（Go 运行时、部分 sandbox）仍会失败。
- 静态 musl 与 glibc 不受益，仍需 L2 或重新编译。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| 仅 L1 源码兼容 | 简单；需重新编译，Alpine 二进制无法直接用。 |
| 仅 L2 Linux ABI | 覆盖广；glibc/静态 syscall 面大，早期成本高。 |
| glibc ABI drop-in | 生态大；符号版本、NSS、TLS、加载器复杂，成本最高。 |
| 动态 musl ABI + L2 兜底 | 先拿下 musl 生态；静态/glibc 后续增量。 |

## 架构一致性检查

- 动态 musl 二进制仍是 POSIX 应用，进程/fd/信号由 POSIX Server 维护。
- syscall 经 Holy musl 后端进入 `hk-abi`，不新增内核 Linux 语义。
- ELF/auxv/TLS 由用户态 loader 与 `hk-crt` 协作；内核提供 Arena、TCB、HMP 与 Handle 机制。
- `dlopen` 的模块使用同一 musl ABI；不加载 glibc `.so`。
- 权限、VFS、Network、Display 与 L1 完全一致。

## 相关

- [ADR-0007：musl 移植 + 薄 hk-libc](0007-posix-libc.md)
- [ADR-0011：Linux 二进制 ABI 兼容层](0011-linux-abi.md)
- [应用兼容与 Linux ABI](../compat.md)
