//! Limine 引导适配。
//!
//! 定义 Limine 请求，并把响应转换为平台无关的 [`BootInfo`]：内存 map、内核镜像、
//! 帧缓冲、direct map、RSDP 与服务器模块。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_base::{
    Arch, BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootModule, DirectMapInfo, FramebufferInfo,
    MemoryRegion, PAGE_SIZE, PhysAddr, PixelFormat, RegionKind, VirtAddr,
};
use limine::BaseRevision;

use crate::BootError;
use limine::memory_map::EntryType;
use limine::mp::Cpu as LimineCpu;
use limine::request::{
    DeviceTreeBlobRequest, ExecutableAddressRequest, ExecutableCmdlineRequest, FramebufferRequest,
    HhdmRequest, MemoryMapRequest, ModuleRequest, MpRequest, RequestsEndMarker,
    RequestsStartMarker, RsdpRequest, StackSizeRequest,
};

/// 目标架构。
const ARCH: Arch = if cfg!(target_arch = "x86_64") {
    Arch::X86_64
} else if cfg!(target_arch = "aarch64") {
    Arch::Aarch64
} else {
    Arch::Riscv64
};

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

/// 内核栈大小：内核对象表与 trace 环形缓冲在启动期占用较多栈空间。
const KERNEL_STACK_SIZE: u64 = 256 * 1024;

#[used]
#[unsafe(link_section = ".limine_requests")]
static STACK_SIZE: StackSizeRequest = StackSizeRequest::new().with_size(KERNEL_STACK_SIZE);

#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMORY_MAP: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static EXECUTABLE_ADDRESS: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static EXECUTABLE_CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static MODULES: ModuleRequest = ModuleRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static FRAMEBUFFER: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static RSDP: RsdpRequest = RsdpRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static DEVICE_TREE_BLOB: DeviceTreeBlobRequest = DeviceTreeBlobRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static MP: MpRequest = MpRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// Limine 引导后端。
#[derive(Clone, Copy, Debug, Default)]
pub struct Limine;

impl crate::BootBackend for Limine {
    fn boot_info(&self) -> Result<BootInfo, BootError> {
        boot_info()
    }

    fn cmdline(&self) -> Option<&'static str> {
        cmdline()
    }
}

/// 读取 Limine 响应并构造启动信息。
pub fn boot_info() -> Result<BootInfo, BootError> {
    if !BASE_REVISION.is_supported() {
        return Err(BootError::UnsupportedBaseRevision {
            actual: BASE_REVISION.loaded_revision().unwrap_or(0),
        });
    }

    let hhdm = HHDM
        .get_response()
        .ok_or(BootError::MissingResponse("hhdm"))?
        .offset();
    let memory_map = MEMORY_MAP
        .get_response()
        .ok_or(BootError::MissingResponse("memory map"))?;

    let executable_address = EXECUTABLE_ADDRESS
        .get_response()
        .ok_or(BootError::MissingResponse("executable address"))?;
    let kernel_base = PhysAddr::new(executable_address.physical_base());

    let mut regions = Vec::with_capacity(memory_map.entries().len());
    let mut highest = 0_u64;
    let mut kernel_region = None;
    for entry in memory_map.entries() {
        if entry.length == 0 {
            continue;
        }
        let region = MemoryRegion::new(
            PhysAddr::new(entry.base),
            entry.length,
            kind_of(entry.entry_type),
        );
        if entry.entry_type == EntryType::EXECUTABLE_AND_MODULES && region.contains(kernel_base) {
            kernel_region = Some(region);
        }
        regions.push(region);
        highest = highest.max(entry.base.saturating_add(entry.length));
    }

    let kernel_region =
        kernel_region.ok_or(BootError::MissingKernelRegion { base: kernel_base })?;
    let kernel_image = MemoryRegion::new(kernel_region.base, kernel_region.len, RegionKind::Kernel);

    let framebuffer = FRAMEBUFFER.get_response().and_then(|response| {
        let framebuffer = response.framebuffers().next()?;
        if framebuffer.bpp() != 32 {
            return None;
        }
        let phys = physical(framebuffer.addr() as u64, hhdm).ok()?;
        Some(FramebufferInfo {
            phys,
            width: framebuffer.width() as u32,
            height: framebuffer.height() as u32,
            stride: framebuffer.pitch() as u32,
            format: if framebuffer.red_mask_shift() == 16 {
                PixelFormat::Bgr32
            } else {
                PixelFormat::Rgb32
            },
        })
    });

    let mut modules = Vec::new();
    if let Some(response) = MODULES.get_response() {
        for file in response.modules() {
            let name = core::str::from_utf8(file.path().to_bytes())
                .map_err(|_| BootError::InvalidModuleName)?;
            modules.push(BootModule {
                name: Box::leak(String::from(name).into_boxed_str()),
                region: MemoryRegion::new(
                    physical(file.addr() as u64, hhdm)?,
                    file.size(),
                    RegionKind::Module,
                ),
            });
        }
    }

    let rsdp = RSDP
        .get_response()
        .map(|response| PhysAddr::new(response.address() as u64));

    let dtb = match DEVICE_TREE_BLOB.get_response() {
        Some(response) => {
            let pointer = response.dtb_ptr() as u64;
            if pointer == 0 {
                None
            } else {
                Some(physical(pointer, hhdm)?)
            }
        }
        None => None,
    };

    let direct_map = Some(DirectMapInfo {
        phys_base: PhysAddr::new(0),
        virt_base: VirtAddr::new(hhdm),
        len: highest.div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64,
    });

    let info = BootInfo {
        magic: BOOT_INFO_MAGIC,
        version: BOOT_INFO_VERSION,
        arch: ARCH,
        memory_map: leak(regions),
        kernel_image,
        framebuffer,
        direct_map,
        dtb,
        rsdp,
        server_modules: leak(modules),
    };
    Ok(info)
}

/// 读取内核命令行。
pub fn cmdline() -> Option<&'static str> {
    let response = EXECUTABLE_CMDLINE.get_response()?;
    let cmdline = response.cmdline();
    core::str::from_utf8(cmdline.to_bytes()).ok()
}

/// AP 描述前缀；与 Limine x86_64 CPU 结构布局一致。
#[cfg(target_arch = "x86_64")]
#[repr(C)]
pub struct ApCpu {
    /// ACPI 处理器编号。
    pub id: u32,
    /// 本地 APIC 编号。
    pub lapic_id: u32,
    _reserved: u64,
    _goto: u64,
    /// 供内核传递 AP 序号。
    pub extra: AtomicU64,
}

/// AP 描述前缀；与 Limine AArch64 CPU 结构布局一致。
#[cfg(target_arch = "aarch64")]
#[repr(C)]
pub struct ApCpu {
    /// ACPI 处理器编号。
    pub id: u32,
    _reserved1: u32,
    /// 处理器 MPIDR。
    pub mpidr: u64,
    _reserved: u64,
    _goto: u64,
    /// 供内核传递 AP 序号。
    pub extra: AtomicU64,
}

/// AP 描述前缀；与 Limine RISC-V CPU 结构布局一致。
#[cfg(target_arch = "riscv64")]
#[repr(C)]
pub struct ApCpu {
    /// 处理器编号。
    pub id: u64,
    /// 硬件线程（hart）编号。
    pub hartid: u64,
    _reserved: u64,
    _goto: u64,
    /// 供内核传递 AP 序号。
    pub extra: AtomicU64,
}

/// AP 入口。
pub type ApEntry = unsafe extern "C" fn(*mut ApCpu) -> !;

/// SMP 概要。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmpInfo {
    /// BSP 硬件标识：x86 APIC ID、AArch64 MPIDR 或 RISC-V hart ID。
    pub bsp_id: u64,
    /// CPU 总数（含 BSP）。
    pub cpu_count: usize,
}

fn bsp_id(response: &limine::response::MpResponse) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        u64::from(response.bsp_lapic_id())
    }
    #[cfg(target_arch = "aarch64")]
    {
        response.bsp_mpidr()
    }
    #[cfg(target_arch = "riscv64")]
    {
        response.bsp_hartid()
    }
}

fn hardware_id(cpu: &LimineCpu) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        u64::from(cpu.lapic_id)
    }
    #[cfg(target_arch = "aarch64")]
    {
        cpu.mpidr
    }
    #[cfg(target_arch = "riscv64")]
    {
        cpu.hartid
    }
}

/// 返回 Limine 的 SMP 概要；没有响应时为 `None`。
pub fn smp_info() -> Option<SmpInfo> {
    let response = MP.get_response()?;
    Some(SmpInfo {
        bsp_id: bsp_id(response),
        cpu_count: response.cpus().len(),
    })
}

/// 为所有非 BSP CPU 写入入口，返回启动的 AP 数量。
pub fn start_aps(entry: ApEntry) -> usize {
    let Some(response) = MP.get_response() else {
        return 0;
    };
    let bsp = bsp_id(response);
    let mut started = 0;
    let mut slot = 1_u64;

    for cpu in response.cpus() {
        if hardware_id(cpu) == bsp {
            continue;
        }
        cpu.extra.store(slot, Ordering::Release);
        // SAFETY: `ApEntry` 与 Limine 回调使用相同的 C ABI，参数指向同一 CPU 结构。
        let callback: unsafe extern "C" fn(&LimineCpu) -> ! =
            unsafe { core::mem::transmute(entry) };
        cpu.goto_address.write(callback);
        started += 1;
        slot += 1;
    }

    started
}

fn kind_of(entry_type: EntryType) -> RegionKind {
    if entry_type == EntryType::USABLE {
        RegionKind::Usable
    } else if entry_type == EntryType::ACPI_RECLAIMABLE {
        RegionKind::AcpiReclaimable
    } else if entry_type == EntryType::BAD_MEMORY {
        RegionKind::BadMemory
    } else if entry_type == EntryType::FRAMEBUFFER {
        RegionKind::Framebuffer
    } else {
        RegionKind::Reserved
    }
}

fn physical(virtual_address: u64, hhdm: u64) -> Result<PhysAddr, BootError> {
    virtual_address
        .checked_sub(hhdm)
        .map(PhysAddr::new)
        .ok_or(BootError::AddressTranslation {
            address: virtual_address,
        })
}

fn leak<T>(values: Vec<T>) -> &'static [T] {
    let leaked: &'static mut [T] = Box::leak(values.into_boxed_slice());
    leaked
}
