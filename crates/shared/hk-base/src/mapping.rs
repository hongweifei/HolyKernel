//! 映射属性：缓存策略。

/// 设备映射的缓存策略。
///
/// 缓存属性属于映射种类的内部属性：谁把物理区间交给任务，谁就知道它是寄存器
/// 窗口、系统内存扫描输出还是显存（见 ADR-0016 §7）。它记录在能力与 Region 上，
/// 不出现在用户可请求的映射标志里。
///
/// 放在 `hk-base` 而不是 `hk-hal`，是因为启动信息 [`crate::FramebufferInfo`] 也
/// 要携带它；`hk-hal` 重新导出同一类型，内核与平台只从那里取。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CachePolicy {
    /// 写回：与普通内存同域。适配 CPU 与设备共享缓存一致性的系统内存，
    /// 例如 QEMU 的线性帧缓冲；写者不需要 `clflush`。
    WriteBack = 0,
    /// 写合并：顺序写由 CPU 攒成整行突发，适合逐行写入的显存扫描输出。
    /// 读回代价高，不确定时不要选。
    WriteCombining = 1,
    /// 非缓存：每次访问直达设备，适合寄存器窗口与必须逐次可见的控制面。
    Uncached = 2,
}

impl CachePolicy {
    /// 数值编码，供只存整数的启动信息与静态量使用。
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// 从数值编码还原；越界返回 `None`。
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::WriteBack),
            1 => Some(Self::WriteCombining),
            2 => Some(Self::Uncached),
            _ => None,
        }
    }
}
