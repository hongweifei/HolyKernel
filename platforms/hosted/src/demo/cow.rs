//! Hosted 写时复制 fork 原型：父地址空间分叉后，子进程写页触发私有副本。

use std::fmt::Write;
use std::sync::Mutex;

use hk_hal::{MapFlags, Mmu, PAGE_SIZE, PhysAddr, PhysRange, PhysicalAccess, VirtAddr};
use hk_mem::{Frame, FrameAllocator, MemError, Untyped, VSpace};
use hk_platform_hosted::mmu::HostedMmu;
use hk_platform_hosted::physical::HostedMemory;

/// 页帧来源。
struct DemoAllocator {
    untyped: Mutex<Untyped>,
}

impl FrameAllocator for DemoAllocator {
    fn allocate(&self, len: usize) -> Result<Frame, MemError> {
        self.untyped
            .lock()
            .expect("页帧分配器锁中毒")
            .retype_frame(len)
    }
}

/// 运行 COW fork 原型，返回报告。
pub fn run() -> Result<String, String> {
    let mmu = HostedMmu::new();
    let memory = HostedMemory::new(PhysRange::new(PhysAddr::new(0x300_0000), 0x10_0000));
    let allocator = DemoAllocator {
        untyped: Mutex::new(Untyped::new(PhysRange::new(
            PhysAddr::new(0x300_0000),
            0x10_0000,
        ))),
    };

    let virt = VirtAddr::new(0x40_0000);
    let parent = VSpace::new(mmu.create_vspace());
    let child = VSpace::new(mmu.create_vspace());

    let frame = allocator
        .allocate(PAGE_SIZE)
        .map_err(|error| format!("分配父页失败：{error}"))?;
    memory
        .write(frame.phys(), b"parent")
        .map_err(|error| format!("写入父页失败：{error}"))?;
    parent
        .map(
            &mmu,
            &frame,
            virt,
            PAGE_SIZE,
            MapFlags::READ | MapFlags::WRITE,
        )
        .map_err(|error| format!("映射父页失败：{error}"))?;
    parent
        .fork_into(&mmu, &child)
        .map_err(|error| format!("复制地址空间失败：{error}"))?;

    if !parent.is_cow(virt) || !child.is_cow(virt) {
        return Err(String::from("fork 后页未标记写时复制"));
    }

    let child_phys = child
        .resolve_write_fault(&mmu, &memory, &allocator, virt)
        .map_err(|error| format!("处理子页写错误失败：{error}"))?;
    memory
        .write(child_phys, b"child!")
        .map_err(|error| format!("写入子页失败：{error}"))?;

    let mut parent_bytes = [0_u8; 6];
    let mut child_bytes = [0_u8; 6];
    memory
        .read(frame.phys(), &mut parent_bytes)
        .map_err(|error| format!("读取父页失败：{error}"))?;
    memory
        .read(child_phys, &mut child_bytes)
        .map_err(|error| format!("读取子页失败：{error}"))?;

    if &parent_bytes != b"parent" {
        return Err(format!("父页被污染：{parent_bytes:?}"));
    }
    if &child_bytes != b"child!" {
        return Err(format!("子页写入失败：{child_bytes:?}"));
    }

    let mut report = String::new();
    writeln!(
        report,
        "COW fork：父页 {:#x}，子页 {:#x}",
        frame.phys().get(),
        child_phys.get()
    )
    .expect("格式化 COW 报告");
    writeln!(report, "COW fork：父子页独立").expect("格式化 COW 报告");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forks_and_copies_on_write() {
        let report = run().expect("COW fork 原型");
        assert!(report.contains("父子页独立"));
    }
}
