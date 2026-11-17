//! 服务 ELF 加载与任务登记。
//!
//! 镜像解析、用户栈与 Arena 布局由 `hk-root-server::loader` 完成；本模块提供
//! x86 用户页映射（`UserMemory`）与启动信息到镜像布局的适配。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alloc::sync::Arc;

use hk_hal::{BootInfo, BootModule, MapFlags, PAGE_SIZE, PhysAddr, UserMemory, VirtAddr};
use hk_mem::FrameBank;
use hk_mem::hmp::Capacity;
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{PageFlags, PageTable};
use hk_root_server::loader::{self, ImageSpec};
use hk_root_server::{ServiceSpec, ServiceTable};
use spin::Mutex;

use crate::host;

/// 用户栈基址。
const USER_STACK_BASE: u64 = hk_base::user::STACK_BASE;

/// 用户栈大小。
const USER_STACK_SIZE: u64 = hk_base::user::STACK_SIZE;

/// 各服务用户栈之间的间隔。
const USER_STACK_STRIDE: u64 = hk_base::user::STACK_STRIDE;

/// 加载默认启动服务并运行到退出。
pub fn run(
    info: &BootInfo,
    direct_map_base: u64,
    allocator: &FrameBank,
    paging: &PageTable,
    services: &mut ServiceTable,
) -> Result<(), String> {
    // 启动集合来自 `hk-catalog`：默认白名单优先，opt-in 服务按目录顺序追加。
    let cmdline = hk_boot::limine::cmdline().unwrap_or("");
    let mut modules = loader::boot_modules(info.server_modules);
    for entry in hk_catalog::SERVICES {
        if entry.boot || !hk_catalog::activated(entry, cmdline) {
            continue;
        }
        if let Some(module) = info
            .server_modules
            .iter()
            .find(|module| loader::service_name(module.name) == entry.name)
        {
            modules.push(module);
        }
    }
    let console_test = cmdline.contains("console-test");
    let desktop = cmdline.contains("desktop");
    // 模式选择同样来自 `hk-catalog`：命中测试/桌面模式时按集合裁剪，
    // 否则按默认白名单隐藏按需服务。
    if let Some(mode) = hk_catalog::boot_mode(cmdline) {
        modules.retain(|module| mode.services.contains(&loader::service_name(module.name)));
    } else {
        let network = hk_catalog::network_enabled(cmdline);
        modules.retain(|module| {
            hk_catalog::default_visible(loader::service_name(module.name), network)
        });
    }

    if desktop {
        // 桌面启动顺序：Display/输入先就绪，Console 最后创建以获得初始焦点。
        modules.sort_by_key(|module| hk_catalog::desktop_order(loader::service_name(module.name)));
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
            let spec = if device {
                ServiceSpec::device(name)
            } else {
                ServiceSpec::new(name)
            };
            let mut spec = spec.with_cadence(hk_catalog::default_cadence(name));
            if hk_catalog::GPU_CAPACITY_SERVICES.contains(name) {
                spec = spec.with_capacity(
                    hk_mem::hmp::CapacityKind::Gpu,
                    hk_catalog::GPU_CAPACITY_BYTES,
                );
            }
            if hk_catalog::policy_authority(name) {
                spec = spec.with_policy_authority();
            }
            spec
        })
        .collect();
    host::init_symbols(info);
    host::init(&specs)?;
    host::register_modules(info, &modules);
    let image_provider = modules
        .iter()
        .position(|module| loader::service_name(module.name) == "hk-server-image");

    for (index, module) in modules.iter().enumerate() {
        let endpoint = index as u64 + 1;
        let (context, regions) = load(
            info,
            module,
            index,
            endpoint,
            direct_map_base,
            allocator,
            paging,
            image_provider,
        )?;
        services.register_with_authority(names[index], specs[index].device_authority);
        host::attach_with_regions(index, context, &regions)?;
    }
    // 服务装载可能在高半区新增页表页；补登记新出现的内核映射。
    hk_platform_x86_64::system_arena::register_kernel_mappings(paging);

    let mut shell_loader = crate::shell::Loader {
        info,
        direct_map_base,
        allocator,
        paging,
    };
    let mut restarts = 0;
    loop {
        let idle = host::run_budgeted(services, 1024);
        host::process_controls(&mut shell_loader, services);
        let restarted = host::supervise(&mut shell_loader, services);
        if restarted > 0 {
            restarts += restarted;
            if restarts >= 64 {
                break;
            }
            continue;
        }
        if idle {
            // `idle` 只表示这一刻没有可被选中的任务，不代表系统已经收尾：
            // 任务可能只是在等实时预算补充、sleep 到期或外部输入。图形桌面与
            // 控制台自检要一直等外部中断；其他模式只要还有未退出任务且存在
            // 下一次唤醒期限，就必须等到期，而不是提前收尾。
            let pending = host::tasks().iter().any(|task| task.state != "exited");
            let keep_alive = desktop || console_test || host::has_pending_wakeup();
            if keep_alive && pending {
                // 在关中断状态下编程唤醒期限，再由 `wait_for_interrupt` 原子地
                // 开中断并 `hlt`。否则一次性唤醒可能在 `hlt` 之前被投递，随后
                // 进入没有定时器的永久等待。
                host::arm_idle_wakeup();
                hk_platform_x86_64::cpu::wait_for_interrupt();
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
    allocator: &FrameBank,
    paging: &PageTable,
    image_provider: Option<usize>,
) -> Result<(cpu::InterruptFrame, Vec<hk_root_server::UserRegionSpec>), String> {
    let bytes = info
        .module_bytes(module)
        .ok_or_else(|| format!("模块不在 direct map 内：{}", module.name))?;

    let stack_base = USER_STACK_BASE + index as u64 * USER_STACK_STRIDE;
    let (arena_base, arena_size) = host::arena(index);
    let lazy_image = hk_catalog::lazy_task_image(loader::service_name(module.name));
    let provider_endpoint = image_provider
        .filter(|_| hk_catalog::user_image_service(loader::service_name(module.name)));
    let module_phys = lazy_image.then(|| PhysAddr::new(bytes.as_ptr() as u64 - direct_map_base));
    let spec = ImageSpec {
        bytes,
        endpoint,
        stack_base,
        stack_size: USER_STACK_SIZE,
        arena_base,
        arena_size,
        module_phys,
        lazy_image,
        provider_endpoint,
        module_index: index,
    };

    let table = paging
        .create_user(allocator)
        .map_err(|error| format!("创建地址空间失败：{error:?}"))?;
    let capacity = hk_platform_x86_64::system::user_capacity()
        .ok_or_else(|| String::from("User Capacity 尚未建立"))?;
    let mut memory = UserPages {
        direct_map_base,
        paging: &table,
        capacity,
    };
    let (context, regions) = loader::load(&spec, &mut memory, |entry, stack_top, endpoint| {
        cpu::user_frame(entry, stack_top, endpoint)
    })
    .map_err(|error| format!("装载服务失败：{}：{error}", module.name))?;
    hk_platform_x86_64::USER_ADDRESS_SPACES
        .install(index, hk_platform_x86_64::UserSpace::new(table));
    Ok((context, regions))
}

struct UserPages<'a> {
    direct_map_base: u64,
    paging: &'a PageTable,
    /// 镜像与用户栈从任务所属的 User Capacity 取页，物理归因不再落到 System。
    capacity: Arc<Mutex<Capacity>>,
}

impl UserMemory for UserPages<'_> {
    fn map_page(&mut self, virtual_address: u64) -> Option<&mut [u8]> {
        let frame = self.capacity.lock().allocate(PAGE_SIZE).ok()?;
        let physical = frame.phys();
        let destination = (self.direct_map_base + physical.get()) as *mut u8;

        // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
        unsafe {
            core::ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        self.paging
            .map(VirtAddr::new(virtual_address), physical, PageFlags::USER_RW)
            .ok()?;
        if !hk_platform_x86_64::paging::mapping_acquire(physical) {
            return None;
        }

        // SAFETY: 帧经 HHDM 映射为整页可写；映射建立后返回该页视图。
        Some(unsafe { core::slice::from_raw_parts_mut(destination, PAGE_SIZE) })
    }

    fn protect_page(&mut self, virtual_address: u64, writable: bool, executable: bool) {
        let mut flags = MapFlags::READ;
        if writable {
            flags |= MapFlags::WRITE;
        }
        if executable {
            flags |= MapFlags::EXECUTE;
        }
        let _ = self.paging.protect_user_range(
            VirtAddr::new(virtual_address),
            PAGE_SIZE,
            PageFlags::from_map(flags),
        );
    }
}

/// 按名称装载服务模块，用于按需启动或热替换。
pub fn load_service(
    info: &BootInfo,
    name: &str,
    slot: usize,
    direct_map_base: u64,
    allocator: &FrameBank,
    paging: &PageTable,
) -> Result<(cpu::InterruptFrame, Vec<hk_root_server::UserRegionSpec>), String> {
    load_module(info, name, slot, direct_map_base, allocator, paging)
}

/// 按模块名装载原生镜像；模块名是去掉 `.hkx` 的服务名。
pub fn load_module(
    info: &BootInfo,
    name: &str,
    slot: usize,
    direct_map_base: u64,
    allocator: &FrameBank,
    paging: &PageTable,
) -> Result<(cpu::InterruptFrame, Vec<hk_root_server::UserRegionSpec>), String> {
    let module = info
        .server_modules
        .iter()
        .find(|module| !loader::is_script(module.name) && loader::service_name(module.name) == name)
        .ok_or_else(|| format!("未知镜像：{name}"))?;
    load(
        info,
        module,
        slot,
        0,
        direct_map_base,
        allocator,
        paging,
        None,
    )
}
