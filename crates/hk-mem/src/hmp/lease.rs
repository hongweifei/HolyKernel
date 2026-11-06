//! FrameLease：类型化能力的物理页持有租约。

use core::sync::atomic::{AtomicUsize, Ordering};

use hk_hal::{PhysAddr, PhysRange};

/// lease 释放时归还物理区间的回调。
static BACKING_RELEASER: AtomicUsize = AtomicUsize::new(0);

/// 注册 FrameLease 回收回调；重复调用会替换旧回调。
pub fn set_frame_lease_releaser(release: fn(PhysAddr, usize)) {
    BACKING_RELEASER.store(release as usize, Ordering::Release);
}

/// 一段由 Capacity 或 DMA 池授予的物理页租约。
pub struct FrameLease {
    phys: PhysAddr,
    len: usize,
}

impl FrameLease {
    /// 构造租约。
    pub const fn new(phys: PhysAddr, len: usize) -> Self {
        Self { phys, len }
    }

    /// 返回起始物理地址。
    pub const fn phys(&self) -> PhysAddr {
        self.phys
    }

    /// 返回字节长度。
    pub const fn len(&self) -> usize {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回物理区间。
    pub const fn range(&self) -> PhysRange {
        PhysRange::new(self.phys, self.len as u64)
    }
}

impl Drop for FrameLease {
    fn drop(&mut self) {
        let release = BACKING_RELEASER.load(Ordering::Acquire);
        if release != 0 {
            // SAFETY: 回调由平台在启动期注册，签名与存储的地址一致。
            let release: fn(PhysAddr, usize) = unsafe { core::mem::transmute(release) };
            release(self.phys, self.len);
        }
    }
}
