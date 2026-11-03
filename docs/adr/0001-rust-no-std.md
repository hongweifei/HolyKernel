# ADR-0001：Rust `no_std` 内核 + Rust 用户态服务器

- 状态：**已接受**
- 日期：2026-11-03
- 关联：[章程](../charter.md)、[总体架构](../architecture.md)、[工程策略](../engineering.md)

## 背景

HolyKernel 需要直接操作硬件、可静态审计的微内核，同时需要大量用户态服务器（Root、VFS、POSIX、驱动）。Rust 同时覆盖裸机与用户态，并提供内存安全与成熟工具链。

## 决策

1. 微内核使用 **Rust `no_std`**，只依赖 `core` 与 `alloc`。
2. 用户态服务器使用 Rust；Hosted 服务器可以使用 `std`。
3. 平台层使用最小内联汇编与架构相关代码。
4. 固定 `stable` 工具链，裸机目标通过 `rustup` 安装预编译 `core`。
5. POSIX 边界提供 **C ABI**；musl 作为兼容环境的主 C libc，`hk-abi`/`hk-crt` 提供 shim、启动与头文件，`hk-libc` 作为薄兼容层。
6. `unsafe` 最小化、分 crate 审计、每个块有 `// SAFETY:` 注释。

## 后果

**收益**

- 内核与服务器共享语言与工具链；IPC 消息与句柄类型可复用。
- 内存安全默认开启；所有权/借用消除大量生命周期错误。
- `cargo`、`clippy`、`cargo-deny`、`Miri`、fuzz 覆盖全栈。
- libc 与服务器可使用相同 FFI 类型，降低 ABI 错误。

**成本与取舍**

- 裸机目标需要 `rustup` 安装对应三元组。
- `unsafe` 边界（MMIO、DMA、页表、上下文切换）仍需严格审计。
- 用户态服务器使用 `std` 时，需要保持 Hosted 与裸机行为一致。

## 备选方案

| 方案 | 取舍 |
| --- | --- |
| C/C++ 全栈 | 生态成熟；内存安全依赖工程纪律。 |
| Rust 内核 + C 服务器 | 便于复用 POSIX 代码；跨语言 ABI 成本增加。 |
| Zig | 裸机生态与工具链成熟度较低。 |

## 架构一致性检查

- 内核与服务器使用 Rust；POSIX 边界使用 C ABI。
- 句柄与 IPC 类型在 Rust 与 libc 之间显式映射。
- Hosted 平台的 `std` 限制在服务器/模拟器内部。

## 相关

- [ADR-0002：微内核](0002-microkernel.md)
- [ADR-0007：musl 与 hk-libc](0007-posix-libc.md)
