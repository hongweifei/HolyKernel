//! 调度类别。

/// 调度类别。
///
/// 选择顺序由 [`CadenceClass::rank`] 决定：数值越小越先被 Beat 选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CadenceClass {
    /// 实时：EDF + CBS，用于显示刷新、音频与输入。
    Realtime = 0,
    /// 低延迟：带虚拟截止期的加权公平队列。
    Latency = 1,
    /// 吞吐：加权虚拟时间，用于编译、索引与后台任务。
    Throughput = 2,
    /// 空闲：每 CPU 的 idle TCB。
    Idle = 3,
}

impl CadenceClass {
    /// 类别数量。
    pub const COUNT: usize = 4;

    /// 返回 ABI 编码。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// 从 ABI 编码还原类别。
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Realtime),
            1 => Some(Self::Latency),
            2 => Some(Self::Throughput),
            3 => Some(Self::Idle),
            _ => None,
        }
    }

    /// 返回选择顺序；数值越小越优先。
    pub const fn rank(self) -> u8 {
        self.as_u8()
    }

    /// 返回类别名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Realtime => "realtime",
            Self::Latency => "latency",
            Self::Throughput => "throughput",
            Self::Idle => "idle",
        }
    }

    /// 判断是否为实时类别。
    pub const fn is_realtime(self) -> bool {
        matches!(self, Self::Realtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_orders_classes() {
        let classes = [
            CadenceClass::Realtime,
            CadenceClass::Latency,
            CadenceClass::Throughput,
            CadenceClass::Idle,
        ];
        for (index, class) in classes.iter().enumerate() {
            assert_eq!(class.as_u8() as usize, index);
            assert_eq!(CadenceClass::from_u8(index as u8), Some(*class));
            assert_eq!(class.rank() as usize, index);
        }
        assert_eq!(CadenceClass::from_u8(4), None);
        assert!(CadenceClass::Realtime < CadenceClass::Latency);
    }
}
