# ADR-0015：Compatibility Server 与多来源 UI

- 状态：已接受
- 日期：2026-11-05
- 关联：[POSIX Personality](0003-posix-personality.md)、[Linux ABI](0011-linux-abi.md)、[musl 动态 ABI](0012-musl-dynamic-abi.md)、[UI](../ui.md)、[应用兼容](../compat.md)

## 背景

Holy 桌面承载原生应用、Linux X11/Wayland 应用与鸿蒙应用；兼容层同时支持 musl 与 glibc 动态库。三类运行时的启动方式、libc 布局与权限模型各不相同（见 ADR-0003、ADR-0011、ADR-0012 与 OpenHarmony standard-system 文档），因此按 personality 组织可以共享桌面与文件系统，同时保持各运行时的契约完整。

## 决策

1. **Compatibility Server 作为 supervisor**。它按应用声明选择 personality：
   - `Posix`：`hk-abi` + `hk-crt` + musl 源码移植；
   - `Linux`：Linux ABI + ELF/动态加载器，运行未修改的 glibc/musl 二进制；
   - `Harmony`：OpenHarmony 标准系统用户态 + Holy KAL/HDI 适配。
2. **每个 personality 运行在独立 Native Task/VSpace**。每个进程归属一个 libc 域；glibc 进程加载 glibc 库，musl 进程加载 musl 库，`dlopen` 命名空间与 TLS 状态随进程隔离。同一桌面并行托管两类应用。
3. **UI 采用 surface/seat 合成模型**。Display 作为纯合成器；原生窗口、Wayland surface、XWayland 与 Harmony 表面进入同一场景图。适配器作为独立服务，通过 surface 协议提交缓冲。
4. **Linux GUI 采用 Wayland + XWayland**。Holy 实现 Wayland compositor；X11 应用经 XWayland 接入。
5. **glibc 通过 Linux ABI 运行**。未修改的 glibc 动态程序使用 `ld-linux`、符号版本、IFUNC、TLS、`dlopen`、`/proc` 与 vDSO；Linux personality 提供 Linux syscall 子集。musl 同时提供源码移植与动态 ABI 两条路径（ADR-0012）。
6. **鸿蒙复用 OpenHarmony 用户态**：复用 standard-system 的 ArkUI、Render Service、Window Manager、`samgr`、`appexecfwk`、`dsoftbus` 等，补齐 KAL/HDF/HDI 到 Holy 的适配。`.hap` 由 Harmony personality 承载；商业 HarmonyOS 闭源组件按可获得的 OpenHarmony 代码范围复用。

## 后果

- UI 先完成 surface/seat 抽象；Workbench 等应用建在该层之上，复用同一套窗口与输入模型。
- Linux 应用与 Wayland/XWayland 依赖 M7/M8 的 POSIX（Unix socket、`SCM_RIGHTS`、`memfd`/shm、`epoll`、futex）与 M9 的 Linux ABI。
- Harmony 用户态通过 Linux/POSIX 适配层启动，再逐步替换为原生 `hk-abi`；HDI 图形/输入/存储新增 Holy 后端。
- glibc/musl 分别维护测试矩阵，按 personality 选择加载器与库搜索路径。
- 安全边界随 personality 增加：每个适配器与外来应用运行在独立 Task 中，由 Handle 与配额约束。

## 备选方案

- X11 server：协议与扩展面大；本项目采用 Wayland compositor + XWayland，复用成熟桥接实现。
- glibc 源码移植：glibc 与 Linux 内核 ABI 紧耦合；本项目采用 Linux ABI 运行未修改二进制，降低维护成本。
- ArkUI 重写：OpenHarmony 用户态已有 ArkUI/Render Service；本项目复用这些组件，通过 KAL/HDI 适配。
- Wayland-only：X11 应用通过 XWayland 覆盖，鸿蒙通过 HDI 覆盖；本项目同时提供两类适配器。

## 架构一致性检查

Compatibility Server 运行在用户态，内核可信基保持 TCB/VSpace/Handle/Endpoint/Frame/IRQ。Display 持有 Framebuffer/Input 授权；外来表面通过 surface 协议进入合成器。既有 Native、POSIX 与 Linux ABI 决策保持有效，本 ADR 补充多 personality 与多来源 UI 的组织方式。
