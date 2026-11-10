//! x86_64 用户态运行时适配：用户现场与平台副作用。
//!
//! 任务表、系统调用、调度循环与启动装配都在 `crates/`；本模块只把
//! `InterruptFrame`、PCI/计时/清零与 Arena 布局接入运行时宿主。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use hk_checkpoint::Page;
use hk_hal::{Console, SyscallFrame};
use hk_platform_x86_64::cpu::{self, FaultAction, InterruptFrame, SyscallAction};
use hk_platform_x86_64::paging;
use hk_platform_x86_64::timing;
use hk_platform_x86_64::{COM1, Serial, USER_ADDRESS_SPACES};
use hk_root_server::{
    CowOutcome, FaultOutcome, RuntimeHost, ServiceSpec, ServiceTable, Switch, TaskInfo,
    UserPlatform,
};

mod control;
use hk_syscall::{SYS_ROOT_CONTROL, SYS_ROOT_CONTROL_DATA, SYS_ROOT_RESULT};

/// 看门狗两次快照之间的墙钟间隔。
const WATCHDOG_INTERVAL_MS: u64 = 3_000;

static WATCHDOG_LAST_MS: AtomicU64 = AtomicU64::new(0);

static FRAMEBUFFER_PRESENT: AtomicBool = AtomicBool::new(false);
static FRAMEBUFFER_PHYS: AtomicU64 = AtomicU64::new(0);
static FRAMEBUFFER_WIDTH: AtomicU32 = AtomicU32::new(0);
static FRAMEBUFFER_HEIGHT: AtomicU32 = AtomicU32::new(0);
static FRAMEBUFFER_STRIDE: AtomicU32 = AtomicU32::new(0);
static FRAMEBUFFER_FORMAT: AtomicU32 = AtomicU32::new(0);

/// 记录启动阶段的帧缓冲描述；无法提供帧缓冲时传入 `None`。
pub fn set_framebuffer(info: Option<hk_hal::FramebufferInfo>) {
    match info {
        Some(framebuffer) => {
            FRAMEBUFFER_PHYS.store(framebuffer.phys.get(), Ordering::Release);
            FRAMEBUFFER_WIDTH.store(framebuffer.width, Ordering::Release);
            FRAMEBUFFER_HEIGHT.store(framebuffer.height, Ordering::Release);
            FRAMEBUFFER_STRIDE.store(framebuffer.stride, Ordering::Release);
            FRAMEBUFFER_FORMAT.store(framebuffer.format.code(), Ordering::Release);
            FRAMEBUFFER_PRESENT.store(true, Ordering::Release);
        }
        None => FRAMEBUFFER_PRESENT.store(false, Ordering::Release),
    }
}

fn framebuffer_info() -> Option<hk_hal::FramebufferInfo> {
    if !FRAMEBUFFER_PRESENT.load(Ordering::Acquire) {
        return None;
    }
    Some(hk_hal::FramebufferInfo {
        phys: hk_hal::PhysAddr::new(FRAMEBUFFER_PHYS.load(Ordering::Acquire)),
        width: FRAMEBUFFER_WIDTH.load(Ordering::Acquire),
        height: FRAMEBUFFER_HEIGHT.load(Ordering::Acquire),
        stride: FRAMEBUFFER_STRIDE.load(Ordering::Acquire),
        format: hk_hal::PixelFormat::from_code(FRAMEBUFFER_FORMAT.load(Ordering::Acquire))?,
    })
}

/// Arena 虚拟基址。
const ARENA_BASE: u64 = hk_base::user::ARENA_BASE;

/// 相邻任务 Arena 的间隔。
const ARENA_STRIDE: u64 = hk_base::user::ARENA_STRIDE;

/// 每个任务的 Arena 大小。
const ARENA_SIZE: u64 = hk_base::user::ARENA_SIZE;

/// 用户可访问地址上限。
const USER_LIMIT: u64 = 0x0000_0080_0000_0000;

fn user_range(address: u64, length: usize) -> bool {
    address >= 0x1000
        && address
            .checked_add(length as u64)
            .is_some_and(|end| end <= USER_LIMIT)
}

pub(crate) struct X86Platform(&'static dyn hk_hal::Platform);

impl UserPlatform for X86Platform {
    fn timer_ticks(&self) -> u64 {
        cpu::uptime_ticks()
    }

    fn civil_time(&self) -> Option<hk_base::CivilTime> {
        hk_platform_x86_64::rtc::civil_time()
    }

    fn boot_cmdline(&self) -> Option<&'static str> {
        hk_boot::limine::cmdline()
    }

    fn take_acpi_events(&self) -> u64 {
        hk_platform_x86_64::bootstrap::take_acpi_events()
    }

    fn set_acpi_listener(&self) {
        hk_platform_x86_64::bootstrap::set_acpi_listener();
    }

    fn temperature_celsius(&self) -> Option<u8> {
        self.0.temperature_celsius()
    }

    fn cpu_topology(&self, cpu: u32) -> Option<hk_hal::CpuTopology> {
        self.0.cpu_topology(hk_hal::CpuId::new(cpu))
    }

    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        let Some(pci) = self.0.pci() else {
            return u32::MAX;
        };
        pci.read32(hk_device::PciAddress::new(bus, device, function, offset))
    }

    fn pci_write(&self, bus: u8, device: u8, function: u8, offset: u8, value: u32) {
        if let Some(pci) = self.0.pci() {
            pci.write32(
                hk_device::PciAddress::new(bus, device, function, offset),
                value,
            );
        }
    }

    fn map_mmio(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        USER_ADDRESS_SPACES.map_mmio(slot, virt, phys, len)
    }

    fn map_user(
        &self,
        slot: usize,
        virt: u64,
        phys: u64,
        len: usize,
        rights: hk_mem::Rights,
        flags: hk_mem::hmp::RegionFlags,
    ) -> bool {
        USER_ADDRESS_SPACES.map_user(slot, virt, phys, len, rights, flags)
    }

    fn unmap_user(&self, slot: usize, virt: u64, len: usize) -> bool {
        USER_ADDRESS_SPACES.unmap_user(slot, virt, len)
    }

    fn page_is_zero(&self, phys: hk_hal::PhysAddr) -> Option<bool> {
        let physical = self.0.physical();
        let mut buffer = [0_u8; 256];
        let mut offset = 0;
        while offset < hk_hal::PAGE_SIZE {
            let address = hk_hal::PhysAddr::new(phys.get() + offset as u64);
            physical.read(address, &mut buffer).ok()?;
            if buffer.iter().any(|byte| *byte != 0) {
                return Some(false);
            }
            offset += buffer.len();
        }
        Some(true)
    }

    fn page_shared(&self, phys: hk_hal::PhysAddr) -> bool {
        hk_platform_x86_64::paging::is_shared_frame(phys)
    }

    fn resolve_cow(&self, address: u64) -> Option<CowOutcome> {
        match hk_platform_x86_64::paging::resolve_cow(address) {
            Some(hk_platform_x86_64::paging::CowResolution::Writable) => Some(CowOutcome::Writable),
            Some(hk_platform_x86_64::paging::CowResolution::Copied(phys)) => {
                Some(CowOutcome::Copied(phys.get()))
            }
            None => None,
        }
    }

    fn take_irqs(&self, pending: &mut [u64; 4]) {
        *pending = hk_platform_x86_64::irq::take_pending();
    }

    fn console_read(&self) -> Option<u8> {
        // 用户态输入驱动接管键盘后，内核不再提供控制台输入。
        if hk_boot::limine::cmdline().is_some_and(|line| line.contains("input-driver")) {
            return None;
        }
        hk_platform_x86_64::keyboard::read().or_else(|| Serial::new(COM1).try_read_byte())
    }

    fn take_console_input(&self) -> bool {
        hk_platform_x86_64::keyboard::take_input()
    }

    fn interrupt_controller(&self) -> Option<&dyn hk_hal::InterruptController> {
        Some(self.0.irq())
    }

    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        self.0.iommu()
    }

    fn cache_ops(&self) -> Option<&dyn hk_hal::CacheOps> {
        self.0.cache()
    }

    fn dma_window(&self, len: usize) -> Option<hk_hal::PhysRange> {
        let buffer = self.0.dma().alloc_coherent(len).ok()?;
        Some(hk_hal::PhysRange::new(buffer.phys(), buffer.len() as u64))
    }

    fn dma_free(&self, range: hk_hal::PhysRange) {
        self.0
            .dma()
            .free_coherent(hk_hal::DmaBuffer::new(range.start(), range.len() as usize));
    }

    fn reserve_capacity(
        &self,
        len: usize,
        kind: hk_mem::hmp::CapacityKind,
    ) -> Option<hk_mem::hmp::Capacity> {
        // 授予用容量从覆盖全部可用内存的 System Capacity 切出；DMA 池只是它
        // 旁边一个已预留的窗口，不再作为其它容量的来源。
        hk_platform_x86_64::system::split_capacity(len, kind)
    }

    fn user_capacity(&self) -> Option<alloc::sync::Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
        hk_platform_x86_64::system::user_capacity()
    }

    fn frame_bank_stats(&self) -> Option<hk_mem::FrameBankStats> {
        Some(hk_platform_x86_64::paging::FRAME_BANK.stats())
    }

    fn ledger_stats(&self) -> Option<Vec<hk_mem::LedgerStats>> {
        Some(hk_platform_x86_64::paging::FRAME_BANK.ledger_stats())
    }

    fn zero(&self, address: u64, length: usize) {
        hk_platform_x86_64::cpu::user_access_begin();
        // SAFETY: Arena 页已映射到当前用户页表，SMAP 由 STAC/CLAC 临时放行。
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, length);
        }
        hk_platform_x86_64::cpu::user_access_end();
    }

    fn user_mapped(&self, address: u64) -> bool {
        user_range(address, 1) && hk_platform_x86_64::paging::is_user_mapped(address)
    }

    fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
        if buffer.is_empty() {
            return true;
        }
        if !user_range(address, buffer.len()) {
            return false;
        }
        hk_platform_x86_64::cpu::user_access_begin();
        // SAFETY: 地址已校验落在用户空间，且当前 CR3 属于调用任务。
        unsafe {
            core::ptr::copy_nonoverlapping(address as *const u8, buffer.as_mut_ptr(), buffer.len());
        }
        hk_platform_x86_64::cpu::user_access_end();
        true
    }

    fn write_user(&self, address: u64, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        if !user_range(address, data.len()) {
            return false;
        }
        hk_platform_x86_64::cpu::user_access_begin();
        // SAFETY: 地址已校验落在用户空间，且当前 CR3 属于调用任务。
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len());
        }
        hk_platform_x86_64::cpu::user_access_end();
        true
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (ARENA_BASE + slot as u64 * ARENA_STRIDE, ARENA_SIZE)
    }

    fn memory_limit(&self, _slot: usize) -> u64 {
        hk_platform_x86_64::dma::POOL_BYTES as u64
    }

    fn activate(&self, slot: usize) {
        USER_ADDRESS_SPACES.activate(slot);
    }

    fn vspace(&self, slot: usize) -> u64 {
        USER_ADDRESS_SPACES.root(slot)
    }

    fn fork(&self, parent: usize, child: usize, cow_ranges: &[(u64, u64)]) -> bool {
        USER_ADDRESS_SPACES.fork(parent, child, cow_ranges)
    }

    fn share_vspace(&self, parent: usize, child: usize) -> bool {
        USER_ADDRESS_SPACES.share(parent, child)
    }

    fn set_thread_pointer(&self, tls: u64) {
        cpu::set_thread_pointer(tls);
    }

    fn timestamp(&self) -> u64 {
        self.0.timer().now().ticks()
    }

    fn tsc_frequency(&self) -> u64 {
        self.0.timer().frequency()
    }

    fn arm_wakeup(&self, deadline: Option<u64>) {
        let Some(at) = deadline else {
            hk_platform_x86_64::cpu::resume_periodic();
            return;
        };
        let frequency = hk_platform_x86_64::timing::tsc_frequency();
        if frequency == 0 {
            return;
        }
        let now = (u128::from(hk_platform_x86_64::timing::rdtsc()) * 1_000_000
            / u128::from(frequency))
        .min(u128::from(u64::MAX)) as u64;
        let delta = at.saturating_sub(now).clamp(1, 10_000);
        hk_platform_x86_64::cpu::arm_oneshot(delta);
    }

    fn framebuffer(&self) -> Option<hk_hal::FramebufferInfo> {
        framebuffer_info()
    }

    fn map_framebuffer(&self, slot: usize, virt: u64) -> Option<u64> {
        let framebuffer = framebuffer_info()?;
        let length = u64::from(framebuffer.stride) * u64::from(framebuffer.height);
        if length == 0
            || !virt.is_multiple_of(hk_hal::PAGE_SIZE as u64)
            || !framebuffer
                .phys
                .get()
                .is_multiple_of(hk_hal::PAGE_SIZE as u64)
        {
            return None;
        }
        USER_ADDRESS_SPACES
            .map_mmio(
                slot,
                virt,
                framebuffer.phys.get(),
                usize::try_from(length).ok()?,
            )
            .then_some(length)
    }

    fn wake_aps(&self) {
        cpu::wake_aps();
    }

    fn capture_pages(&self, slot: usize) -> Option<Vec<Page>> {
        USER_ADDRESS_SPACES.with(slot, |table| {
            let mut pages = Vec::new();
            table.user_pages(|virt, flags, phys| {
                let mut data = alloc::vec![0_u8; hk_hal::PAGE_SIZE];
                table.read_phys(hk_hal::PhysAddr::new(phys), &mut data);
                if data.iter().all(|byte| *byte == 0) {
                    data.clear();
                }
                let flags = if table.is_shared_frame(hk_hal::PhysAddr::new(phys)) {
                    flags | hk_checkpoint::PAGE_FLAG_SHARED
                } else {
                    flags
                };
                pages.push(Page { virt, flags, data });
            });
            if pages.is_empty() { None } else { Some(pages) }
        })?
    }

    fn restore_pages(&self, slot: usize, pages: &[Page]) -> bool {
        paging::kernel_table().activate();
        if let Some(old) = USER_ADDRESS_SPACES.take(slot) {
            paging::release_user(&old);
        }
        let template = paging::kernel_table();
        let Ok(table) = template.create_user_with(paging::take_restore_page) else {
            return false;
        };
        for page in pages {
            let Some(frame) = paging::take_restore_page() else {
                return false;
            };
            if page.data.is_empty() {
                table.zero_phys(frame);
            } else {
                table.write_phys(frame, &page.data);
            }
            let flags = paging::restore_flags(page.flags);
            if !paging::mapping_acquire(frame) {
                return false;
            }
            if table
                .map_with(
                    hk_hal::VirtAddr::new(page.virt),
                    frame,
                    flags,
                    paging::take_restore_page,
                )
                .is_err()
            {
                paging::mapping_release(frame);
                return false;
            }
        }
        USER_ADDRESS_SPACES.install(slot, hk_platform_x86_64::UserSpace::new(table));
        true
    }

    fn release_pages(&self, slot: usize) {
        paging::kernel_table().activate();
        if let Some(old) = USER_ADDRESS_SPACES.take(slot) {
            paging::release_user(&old);
        }
    }
}

type Runtime = RuntimeHost<InterruptFrame, X86Platform>;

static RUNTIME: Runtime = RuntimeHost::new();

/// 返回任务槽位的 Arena 基址与大小。
pub fn arena(slot: usize) -> (u64, u64) {
    X86Platform(&hk_platform_x86_64::platform::PLATFORM).arena_layout(slot)
}

/// 解析镜像符号表；平台只负责定位模块字节。
pub fn init_symbols(info: &hk_hal::BootInfo) {
    let Some(module) = info
        .server_modules
        .iter()
        .find(|module| hk_root_server::loader::module_name(module.name) == "kernel.sym")
    else {
        return;
    };
    let Some(bytes) = info.module_bytes(module) else {
        return;
    };
    RUNTIME.install_symbols(bytes);
}

/// 按服务清单装配用户态运行时。
pub fn init(specs: &[ServiceSpec]) -> Result<(), String> {
    RUNTIME.boot(
        specs,
        X86Platform(&hk_platform_x86_64::platform::PLATFORM),
        |_index, endpoint| cpu::user_frame(0, 0, endpoint),
    )?;
    let online = 1 + hk_smp::AP_POOL.online();
    RUNTIME.set_resched_hook(resched);
    RUNTIME.set_online_cpus(online);
    RUNTIME.enable_idle_cpus(online);

    cpu::set_syscall_hook(dispatch);
    cpu::set_preempt_hook(preempt);
    cpu::set_fault_hook(fault);
    Ok(())
}

/// 返回运行时宿主。
pub(crate) fn host() -> &'static RuntimeHost<InterruptFrame, X86Platform> {
    &RUNTIME
}

/// 设置任务的用户态初始现场。
pub fn attach(index: usize, context: InterruptFrame) {
    RUNTIME.attach(index, context);
}

/// 生成任务视图快照。
pub fn tasks() -> Vec<TaskInfo> {
    RUNTIME.tasks()
}

/// 输出任务的 Cadence 契约。
pub fn cadence_text(name: &str) -> String {
    RUNTIME.cadence_text(name)
}

/// 输出任务的 Meter 计量。
pub fn meter_text(name: &str) -> String {
    RUNTIME.meter_text(name)
}

/// 输出 Tempo 策略池。
pub fn tempo_text() -> String {
    RUNTIME.tempo_text()
}

/// 在 stop_tick 之后重新编程 idle 的一次性 deadline。
pub fn arm_idle_wakeup() {
    cpu::enable_wakeup_timer();
    RUNTIME.arm_wakeup(0);
}

/// 是否有尚未到期的休眠任务。
pub fn has_sleepers() -> bool {
    RUNTIME.has_sleepers()
}

/// 返回最近的内核 trace 事件。
pub fn trace(limit: usize) -> Vec<hk_root_server::TraceEvent> {
    RUNTIME.trace(limit)
}

/// 返回运行时统计。
pub fn stats() -> hk_root_server::RuntimeStats {
    RUNTIME.stats()
}

/// 调度至多 `steps` 步；返回是否已无可运行线程。
pub fn run_budgeted(table: &mut ServiceTable, steps: usize) -> bool {
    cpu::start_tick();
    let idle = RUNTIME.run_cpu_budget(0, Some(table), steps);
    cpu::stop_tick();
    if !idle && watchdog_enabled() {
        watchdog_snapshot();
    }
    idle
}

/// 是否通过 `watchdog` 启动参数开启活性看门狗。
fn watchdog_enabled() -> bool {
    hk_boot::limine::cmdline().is_some_and(|line| line.contains("watchdog"))
}

/// 系统未空闲但持续运行时，周期性输出任务快照，便于定位活性停顿。
fn watchdog_snapshot() {
    let frequency = timing::tsc_frequency();
    if frequency == 0 {
        return;
    }
    let now = ((u128::from(timing::rdtsc()) * 1000) / u128::from(frequency)) as u64;
    let last = WATCHDOG_LAST_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < WATCHDOG_INTERVAL_MS {
        return;
    }
    WATCHDOG_LAST_MS.store(now, Ordering::Relaxed);

    let console = Serial::new(COM1);
    let stats = RUNTIME.stats();
    console.write_str(&format!(
        "[watchdog] t={now}ms resumes={:?}\n",
        stats.resumes
    ));
    for task in RUNTIME.tasks() {
        console.write_str(&format!(
            "[watchdog] {} tcb={} state={} pending={} handles={}\n",
            task.name, task.tcb, task.state, task.pending, task.handles
        ));
    }
}

/// AP 调度循环：无任务时关闭 tick 并等待唤醒 IPI。
pub fn run_ap(cpu: usize) -> ! {
    loop {
        RUNTIME.run_cpu(cpu, None);
        if RUNTIME.has_deadline(cpu) {
            cpu::enable_wakeup_timer();
            RUNTIME.arm_wakeup(cpu);
        } else {
            cpu::stop_tick();
        }
        cpu::idle(cpu);
        cpu::start_ap_tick();
    }
}

/// 处理 Root Server 控制请求。
pub fn process_controls(loader: &mut crate::shell::Loader<'_>, table: &mut ServiceTable) {
    control::process(loader, table);
}

/// 按重启策略监督服务；返回重启数量。
pub fn supervise(loader: &mut crate::shell::Loader<'_>, table: &mut ServiceTable) -> usize {
    with_tick(|| {
        hk_root_server::service::supervise(
            &RUNTIME,
            table,
            loader,
            hk_root_server::service::restart_policy,
        )
    })
}

/// 在调度 tick 可用的前提下执行一次服务控制。
///
/// shell 命令在 `run_budgeted` 之外执行，此时 tick 默认关闭；嵌套运行的服务需要
/// 周期 tick 才能推进定时器与抢占。已经开启时保持原状态。
fn with_tick<R, F: FnOnce() -> R>(work: F) -> R {
    let armed = cpu::tick_enabled();
    if !armed {
        cpu::rearm_tick();
    }
    let result = work();
    if !armed {
        cpu::stop_tick();
    }
    result
}

/// 启动服务。
pub fn start_service(
    loader: &mut crate::shell::Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<&str>,
) -> String {
    with_tick(|| hk_root_server::service::start(&RUNTIME, services, loader, name, parent))
}

/// 重启已退出的服务。
pub fn restart_service(
    loader: &mut crate::shell::Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<&str>,
) -> String {
    with_tick(|| hk_root_server::service::restart(&RUNTIME, services, loader, name, parent))
}

/// 保存 Native Task 检查点并返回序列化快照。
pub fn checkpoint_service(services: &mut ServiceTable, name: &str) -> Vec<u8> {
    match RUNTIME.checkpoint(name) {
        Ok(size) => {
            services.mark_exited(name, 0);
            format!("已检查点：{name} {size}\n").into_bytes()
        }
        Err(error) => format!("检查点失败：{error}\n").into_bytes(),
    }
}

/// 恢复内存中的 Native Task 检查点。
pub fn restore_checkpoint_service(services: &mut ServiceTable, name: &str) -> Vec<u8> {
    match RUNTIME.restore_checkpoint(name) {
        Ok(_) => {
            services.mark_running(name);
            format!("已恢复：{name}\n").into_bytes()
        }
        Err(error) => format!("恢复失败：{error}\n").into_bytes(),
    }
}

/// 从序列化快照恢复 Native Task。
pub fn restore_checkpoint_bytes(services: &mut ServiceTable, bytes: &[u8]) -> Vec<u8> {
    match RUNTIME.restore_image(bytes) {
        Ok((name, _slot)) => {
            services.mark_running(&name);
            format!("已恢复：{name}\n").into_bytes()
        }
        Err(error) => format!("恢复失败：{error}\n").into_bytes(),
    }
}

/// 读取内存快照的一段镜像。
pub fn checkpoint_chunk(name: &str, offset: u64, length: usize) -> Vec<u8> {
    RUNTIME
        .checkpoint_chunk(name, offset, length)
        .unwrap_or_default()
}

/// 用另一原生镜像热替换服务。
pub fn replace_service(
    loader: &mut crate::shell::Loader<'_>,
    services: &mut ServiceTable,
    argument: &str,
) -> String {
    let mut parts = argument.split_whitespace();
    let Some(name) = parts.next() else {
        return String::from("用法：replace <服务> <镜像>\n");
    };
    let Some(module) = parts.next() else {
        return String::from("用法：replace <服务> <镜像>\n");
    };
    with_tick(|| hk_root_server::service::replace(&RUNTIME, services, loader, name, module, None))
}

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    if frame.number() == SYS_ROOT_CONTROL_DATA {
        let packed = frame.arg2() as usize;
        let name_len = packed & 0xFFFF;
        let data_len = packed >> 16;
        if name_len > 128 || data_len > 64 * 1024 {
            frame.set_result(u64::MAX);
            return SyscallAction::Continue;
        }
        let mut buffer = alloc::vec![0_u8; name_len + data_len];
        if !RUNTIME.read_user(frame.arg1(), &mut buffer) {
            frame.set_result(u64::MAX);
            return SyscallAction::Continue;
        }
        let Ok(name) = core::str::from_utf8(&buffer[..name_len]) else {
            frame.set_result(u64::MAX);
            return SyscallAction::Continue;
        };
        let data = buffer[name_len..].to_vec();
        let caller = RUNTIME
            .current_task_name(cpu::cpu_index())
            .unwrap_or_default();
        frame.set_result(control::request_data(frame.arg0(), name, data, &caller));
        return SyscallAction::Continue;
    }
    if frame.number() == SYS_ROOT_CONTROL {
        let length = frame.arg2() as usize;
        let mut argument = [0_u8; 128];
        if length > argument.len() || !RUNTIME.read_user(frame.arg1(), &mut argument[..length]) {
            frame.set_result(u64::MAX);
            return SyscallAction::Continue;
        }
        let caller = RUNTIME
            .current_task_name(cpu::cpu_index())
            .unwrap_or_default();
        let argument = core::str::from_utf8(&argument[..length]).unwrap_or("");
        frame.set_result(control::request(frame.arg0(), argument, &caller));
        return SyscallAction::Continue;
    }
    if frame.number() == SYS_ROOT_RESULT {
        let Some(data) = control::take_result(frame.arg0()) else {
            frame.set_result(u64::MAX);
            return SyscallAction::Continue;
        };
        let length = (frame.arg2() as usize).min(data.len());
        if !RUNTIME.write_user(frame.arg1(), &data[..length]) {
            frame.set_result(u64::MAX - 1);
            return SyscallAction::Continue;
        }
        frame.set_result(length as u64);
        return SyscallAction::Continue;
    }
    match RUNTIME.dispatch(cpu::cpu_index(), frame) {
        Switch::Continue => SyscallAction::Continue,
        Switch::Block => SyscallAction::Block,
        Switch::Exit => SyscallAction::Exit,
    }
}

fn preempt(frame: &InterruptFrame) -> bool {
    RUNTIME.preempt(cpu::cpu_index(), *frame)
}

fn resched(cpu: usize) {
    cpu::reschedule(cpu);
}

fn fault(frame: &mut InterruptFrame) -> FaultAction {
    if frame.vector == 14 && frame.error & 0b1 == 0 {
        match RUNTIME.handle_fault(cpu::cpu_index(), cpu::fault_address(), *frame) {
            FaultOutcome::Blocked => return FaultAction::Block,
            FaultOutcome::Resumed => return FaultAction::Resume,
            FaultOutcome::Unhandled => {}
        }
    }
    if frame.vector == 14
        && frame.error & 0b11 == 0b11
        && RUNTIME.resolve_cow(cpu::cpu_index(), cpu::fault_address())
    {
        return FaultAction::Resume;
    }

    let console = Serial::new(COM1);
    console.write_str(&format!(
        "用户异常：{}（CPU {}）cr2={:#x} rip={:#x}\n",
        cpu::exception_name(frame.vector),
        cpu::cpu_index(),
        cpu::fault_address(),
        frame.rip
    ));
    RUNTIME.deliver_fault(
        cpu::cpu_index(),
        hk_root_server::debug::FaultContext {
            kind: cpu::exception_name(frame.vector),
            vector: frame.vector,
            error: frame.error,
            address: cpu::fault_address(),
            rip: frame.rip,
            rsp: frame.rsp,
            rbp: frame.rbp,
        },
    );
    RUNTIME.fault(cpu::cpu_index(), 0xE000 | frame.vector);
    FaultAction::Exit
}
