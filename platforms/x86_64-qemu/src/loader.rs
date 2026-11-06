//! 服务 ELF 加载与任务登记。
//!
//! 镜像解析、用户栈与 Arena 布局由 `hk-root-server::loader` 完成；本模块提供
//! x86 用户页映射（`UserMemory`）与启动信息到镜像布局的适配。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{BootInfo, BootModule, PAGE_SIZE, UserMemory, VirtAddr};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, PageFlags, PageTable};
use hk_root_server::loader::{self, ImageSpec};
use hk_root_server::{ServiceSpec, ServiceTable};

use crate::runtime;

/// 用户栈基址。
const USER_STACK_BASE: u64 = 0x0400_0000;

/// 用户栈大小。
const USER_STACK_SIZE: u64 = 512 * 1024;

/// 各服务用户栈之间的间隔。
const USER_STACK_STRIDE: u64 = 2 * 1024 * 1024;

/// 加载默认启动服务并运行到退出。
pub fn run(
    info: &BootInfo,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
    services: &mut ServiceTable,
) -> Result<(), String> {
    let mut modules = loader::boot_modules(info.server_modules);
    let dbg_test = hk_boot::cmdline().is_some_and(|line| line.contains("dbg-test"));
    if dbg_test
        && let Some(module) = info
            .server_modules
            .iter()
            .find(|module| loader::service_name(module.name) == "hk-server-dbg")
    {
        modules.push(module);
    }
    let network_enabled = hk_boot::cmdline().is_some_and(|line| line.contains("net-test"));
    let console_test = hk_boot::cmdline().is_some_and(|line| line.contains("console-test"));
    let display_test = hk_boot::cmdline().is_some_and(|line| line.contains("display-test"));
    let holydoc_test = hk_boot::cmdline().is_some_and(|line| line.contains("holydoc-test"));
    let debug_test = hk_boot::cmdline()
        .is_some_and(|line| line.contains("debug-test") || line.contains("debug-crash"));
    let display_restart_test =
        hk_boot::cmdline().is_some_and(|line| line.contains("display-restart-test"));
    let hotswap_test = hk_boot::cmdline().is_some_and(|line| line.contains("hotswap-test"));
    let checkpoint_test = hk_boot::cmdline().is_some_and(|line| line.contains("checkpoint-test"));
    let checkpoint_srv_test =
        hk_boot::cmdline().is_some_and(|line| line.contains("checkpoint-srv-test"));
    let desktop = hk_boot::cmdline().is_some_and(|line| line.contains("desktop"));
    // `netstat` 只按需启动（`net` 命令），不在启动时装载，避免非网络模式出现
    // “找不到 Network Server”的噪声与额外任务。
    modules.retain(|module| {
        let name = loader::service_name(module.name);
        if dbg_test {
            // 调试客户端自检只装载客户端本身，结果经 Root 控制读取。
            return name == "hk-server-dbg";
        }
        if console_test {
            // 控制台自检使用最小服务集，避免其他常驻服务让空闲判定永不成立。
            return matches!(name, "hk-server-console" | "hk-server-console-test");
        }
        if display_test {
            // 显示自检只需要 Display 服务与客户端。
            return matches!(name, "hk-server-display" | "hk-server-display-test");
        }
        if holydoc_test {
            // HolyDoc 自检需要 Display、文档服务与提供磁盘文档的 HolyFS 链。
            return matches!(
                name,
                "hk-server-display"
                    | "hk-server-holydoc"
                    | "hk-server-holyfs"
                    | "hk-server-vfs"
                    | "hk-server-virtio"
                    | "hk-server-block"
                    | "hk-server-device"
            );
        }
        if debug_test {
            // Debug 自检只需要 Display 与调试服务。
            return matches!(name, "hk-server-display" | "hk-server-debug");
        }
        if display_restart_test {
            // 热重启自检：Display + Console + 重启客户端。
            return matches!(
                name,
                "hk-server-display" | "hk-server-console" | "hk-server-display-restart-test"
            );
        }
        if hotswap_test {
            // 热替换自检：只装载被替换服务与替换镜像。
            return matches!(
                name,
                "hk-server-echo" | "hk-server-spin" | "hk-server-fault"
            );
        }
        if checkpoint_test {
            // 检查点自检只需要一个阻塞任务与内核 Shell。
            return name == "hk-server-echo";
        }
        if checkpoint_srv_test {
            // Checkpoint Server 自检：服务、目标任务与客户端。
            return matches!(
                name,
                "hk-server-echo" | "hk-server-checkpoint" | "hk-server-checkpoint-test"
            );
        }
        if desktop {
            // 图形桌面：Display + Console + Input/Device + 块驱动 + VFS 文档 + HolyDoc 欢迎页。
            return matches!(
                name,
                "hk-server-display"
                    | "hk-server-console"
                    | "hk-server-device"
                    | "hk-server-input"
                    | "hk-server-virtio"
                    | "hk-server-holyfs"
                    | "hk-server-vfs"
                    | "hk-server-path"
                    | "hk-server-holydoc"
                    | "hk-server-shell"
            );
        }
        if hk_boot::cmdline().is_some_and(|line| line.contains("pointer-test")) {
            // 指针自检：Display + Device + Input + 指针客户端。
            return matches!(
                name,
                "hk-server-display"
                    | "hk-server-device"
                    | "hk-server-input"
                    | "hk-server-pointer-test"
            );
        }
        if hk_boot::cmdline().is_some_and(|line| line.contains("input-test")) {
            // 输入驱动自检：Display + Console + 终端客户端 + Input。
            return matches!(
                name,
                "hk-server-display"
                    | "hk-server-device"
                    | "hk-server-console"
                    | "hk-server-terminal-test"
                    | "hk-server-input"
            );
        }
        if hk_boot::cmdline().is_some_and(|line| line.contains("terminal-test")) {
            // 图形终端自检需要 Console、Display 与客户端。
            return matches!(
                name,
                "hk-server-display" | "hk-server-console" | "hk-server-terminal-test"
            );
        }
        if matches!(
            name,
            "hk-server-netstat"
                | "hk-server-holydoc"
                | "hk-server-debug"
                | "hk-server-shell"
                | "hk-server-display-restart-test"
                | "hk-server-checkpoint"
                | "hk-server-checkpoint-test"
        ) {
            return false;
        }
        if matches!(
            name,
            "hk-server-console"
                | "hk-server-console-test"
                | "hk-server-display"
                | "hk-server-display-test"
                | "hk-server-terminal-test"
                | "hk-server-input"
                | "hk-server-pointer-test"
        ) {
            return false;
        }
        network_enabled
            || !matches!(
                name,
                "hk-server-virtio-net" | "hk-server-net" | "hk-server-netsock"
            )
    });
    if desktop {
        // 桌面启动顺序：Display/输入先就绪，Console 最后创建以获得初始焦点。
        modules.sort_by_key(|module| match loader::service_name(module.name) {
            "hk-server-display" => 0,
            "hk-server-device" => 1,
            "hk-server-input" => 2,
            "hk-server-virtio" => 3,
            "hk-server-holyfs" => 4,
            "hk-server-vfs" => 5,
            "hk-server-path" => 6,
            "hk-server-holydoc" => 7,
            "hk-server-console" => 8,
            "hk-server-shell" => 9,
            _ => 9,
        });
    }
    if modules.is_empty() {
        return Err(String::from("没有服务模块"));
    }

    let names: Vec<&'static str> = modules
        .iter()
        .map(|module| loader::service_name(module.name))
        .collect();
    let specs: Vec<ServiceSpec> = modules
        .iter()
        .zip(names.iter())
        .map(|(module, name)| {
            let bytes = info.module_bytes(module);
            let device = bytes.is_some_and(loader::service_has_device_authority);
            let priority = bytes.map(loader::service_priority).unwrap_or(16);
            let spec = if device {
                ServiceSpec::device(name)
            } else {
                ServiceSpec::new(name)
            };
            spec.with_priority(priority)
        })
        .collect();
    runtime::init_symbols(info);
    runtime::init(&specs)?;

    for (index, module) in modules.iter().enumerate() {
        let endpoint = index as u64 + 1;
        let context = load(
            info,
            module,
            index,
            endpoint,
            direct_map_base,
            allocator,
            paging,
        )?;
        services.register_with_authority(names[index], specs[index].device_authority);
        runtime::attach(index, context);
    }

    let mut shell_loader = crate::shell::Loader {
        info,
        direct_map_base,
        allocator,
        paging,
    };
    let mut restarts = 0;
    loop {
        let idle = runtime::run_budgeted(services, 1024);
        runtime::process_controls(&mut shell_loader, services);
        let restarted = runtime::supervise(&mut shell_loader, services);
        if restarted > 0 {
            restarts += restarted;
            if restarts >= 64 {
                break;
            }
            continue;
        }
        if idle {
            // 控制台自检需要等待外部键盘输入：任务全部阻塞时不要直接退出，
            // 而是进入可被中断唤醒的空闲等待，直到所有任务真正退出。
            if console_test && !runtime::tasks().iter().all(|task| task.state == "exited") {
                // SAFETY: 等待外部中断（键盘）唤醒；`sti` 延迟到下一条指令，
                // `hlt` 不会丢失已挂起的中断。
                unsafe {
                    core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                }
                continue;
            }
            break;
        }
    }
    Ok(())
}

fn load(
    info: &BootInfo,
    module: &BootModule,
    index: usize,
    endpoint: u64,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
) -> Result<cpu::InterruptFrame, String> {
    let bytes = info
        .module_bytes(module)
        .ok_or_else(|| format!("模块不在 direct map 内：{}", module.name))?;

    let stack_base = USER_STACK_BASE + index as u64 * USER_STACK_STRIDE;
    let (arena_base, arena_size) = runtime::arena(index);
    let spec = ImageSpec {
        bytes,
        endpoint,
        stack_base,
        stack_size: USER_STACK_SIZE,
        arena_base,
        arena_size,
    };

    let table = paging
        .create_user(allocator)
        .map_err(|error| format!("创建地址空间失败：{error:?}"))?;
    let mut memory = UserPages {
        direct_map_base,
        paging: &table,
        allocator,
    };
    let context = loader::load(&spec, &mut memory, |entry, stack_top, endpoint| {
        cpu::user_frame(entry, stack_top, endpoint)
    })
    .map_err(|error| format!("装载服务失败：{}：{error}", module.name))?;
    hk_platform_x86_64::USER_VSPACES.install(index, table);
    Ok(context)
}

struct UserPages<'a> {
    direct_map_base: u64,
    paging: &'a PageTable,
    allocator: &'a mut FrameAllocator,
}

impl UserMemory for UserPages<'_> {
    fn map_page(&mut self, virtual_address: u64) -> Option<&mut [u8]> {
        let frame = self.allocator.allocate()?;
        let destination = (self.direct_map_base + frame.get()) as *mut u8;

        // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
        unsafe {
            core::ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        self.paging
            .map(
                VirtAddr::new(virtual_address),
                frame,
                PageFlags::USER_RW,
                self.allocator,
            )
            .ok()?;

        // SAFETY: 帧经 HHDM 映射为整页可写；映射建立后返回该页视图。
        Some(unsafe { core::slice::from_raw_parts_mut(destination, PAGE_SIZE) })
    }
}

/// 按名称装载服务模块，用于按需启动或热替换。
pub fn load_service(
    info: &BootInfo,
    name: &str,
    slot: usize,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
) -> Result<cpu::InterruptFrame, String> {
    load_module(info, name, slot, direct_map_base, allocator, paging)
}

/// 按模块名装载原生镜像；模块名是去掉 `.hkx` 的服务名。
pub fn load_module(
    info: &BootInfo,
    name: &str,
    slot: usize,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
) -> Result<cpu::InterruptFrame, String> {
    let module = info
        .server_modules
        .iter()
        .find(|module| !loader::is_script(module.name) && loader::service_name(module.name) == name)
        .ok_or_else(|| format!("未知镜像：{name}"))?;
    load(info, module, slot, 0, direct_map_base, allocator, paging)
}
