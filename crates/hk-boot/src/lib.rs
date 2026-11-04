//! Limine 引导适配。
//!
//! 定义 Limine 请求，并把响应转换为平台无关的 [`BootInfo`]：内存 map、内核镜像、
//! 帧缓冲、direct map、RSDP 与服务器模块。

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{
    Arch, BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootInfoError, BootModule, DirectMapInfo,
    FramebufferInfo, MemoryRegion, PAGE_SIZE, PhysAddr, PixelFormat, RegionKind, VirtAddr,
};
use limine::BaseRevision;
use limine::memory_map::EntryType;
use limine::mp::Cpu as LimineCpu;
use limine::request::{
    ExecutableAddressRequest, ExecutableCmdlineRequest, FramebufferRequest, HhdmRequest,
    MemoryMapRequest, ModuleRequest, MpRequest, RequestsEndMarker, RequestsStartMarker,
    RsdpRequest, StackSizeRequest,
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
static MP: MpRequest = MpRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// 引导适配错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootError {
    /// Limine 没有回应某类请求。
    MissingResponse(&'static str),
    /// Limine 不支持请求的基础协议版本。
    UnsupportedBaseRevision {
        /// 实际生效的版本。
        actual: u64,
    },
    /// 地址无法从 HHDM 虚拟地址换算为物理地址。
    AddressTranslation {
        /// 出错的虚拟地址。
        address: u64,
    },
    /// 模块路径不是合法 UTF-8。
    InvalidModuleName,
    /// 内存 map 中没有描述内核镜像所在区域。
    MissingKernelRegion {
        /// 内核镜像起始物理地址。
        base: PhysAddr,
    },
    /// 启动信息校验失败。
    InvalidBootInfo(BootInfoError),
}

impl fmt::Display for BootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingResponse(name) => write!(formatter, "Limine 未回应 {name} 请求"),
            Self::UnsupportedBaseRevision { actual } => {
                write!(formatter, "Limine 基础协议版本不受支持：{actual}")
            }
            Self::AddressTranslation { address } => {
                write!(formatter, "地址 {address:#x} 无法换算为物理地址")
            }
            Self::InvalidModuleName => formatter.write_str("模块路径不是合法 UTF-8"),
            Self::MissingKernelRegion { base } => {
                write!(formatter, "内存 map 未描述内核镜像区域：{base:?}")
            }
            Self::InvalidBootInfo(error) => write!(formatter, "启动信息校验失败：{error}"),
        }
    }
}

impl core::error::Error for BootError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::InvalidBootInfo(error) => Some(error),
            _ => None,
        }
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

    let direct_map = Some(DirectMapInfo {
        phys_base: PhysAddr::new(0),
        virt_base: VirtAddr::new(hhdm),
        len: highest.div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64,
    });

    let info = BootInfo {
        magic: BOOT_INFO_MAGIC,
        version: BOOT_INFO_VERSION,
        arch: Arch::X86_64,
        memory_map: leak(regions),
        kernel_image,
        framebuffer,
        direct_map,
        dtb: None,
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

/// AP 描述前缀；与 Limine CPU 结构前两个字段一致。
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

/// AP 入口。
pub type ApEntry = unsafe extern "C" fn(*mut ApCpu) -> !;

/// SMP 概要。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmpInfo {
    /// BSP 的本地 APIC 编号。
    pub bsp_lapic_id: u32,
    /// CPU 总数（含 BSP）。
    pub cpu_count: usize,
}

/// 返回 Limine 的 SMP 概要；没有响应时为 `None`。
pub fn smp_info() -> Option<SmpInfo> {
    let response = MP.get_response()?;
    Some(SmpInfo {
        bsp_lapic_id: response.bsp_lapic_id(),
        cpu_count: response.cpus().len(),
    })
}

/// 为所有非 BSP CPU 写入入口，返回启动的 AP 数量。
pub fn start_aps(entry: ApEntry) -> usize {
    let Some(response) = MP.get_response() else {
        return 0;
    };
    let bsp = response.bsp_lapic_id();
    let mut started = 0;

    for (index, cpu) in response.cpus().iter().enumerate() {
        if cpu.lapic_id == bsp {
            continue;
        }
        cpu.extra.store(index as u64, Ordering::Release);
        // SAFETY: `ApEntry` 与 Limine 回调使用相同的 C ABI，参数指向同一 CPU 结构。
        let callback: unsafe extern "C" fn(&LimineCpu) -> ! =
            unsafe { core::mem::transmute(entry) };
        cpu.goto_address.write(callback);
        started += 1;
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
