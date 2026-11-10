//! 类型化能力与 Mappable 接口。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_handle::ObjectId;

use super::{HmpError, ProviderId};

pub use hk_syscall::CapabilityKind;

static NEXT_CAPABILITY_ID: AtomicU64 = AtomicU64::new(1);

/// 能力类型对类型化操作的判定；编码与名称来自 ABI。
trait CapabilityKindExt {
    fn supports(self, operation: CapabilityOp) -> bool;
}

impl CapabilityKindExt for CapabilityKind {
    fn supports(self, operation: CapabilityOp) -> bool {
        match operation {
            CapabilityOp::Map => true,
            CapabilityOp::Commit => matches!(self, Self::Revision),
            CapabilityOp::Scanout => matches!(self, Self::Surface),
            CapabilityOp::Execute => matches!(self, Self::TaskImage | Self::Frame),
            CapabilityOp::Pin => {
                matches!(self, Self::Surface | Self::DeviceAllocation | Self::Frame)
            }
            CapabilityOp::Export => matches!(self, Self::DeviceAllocation | Self::Frame),
            CapabilityOp::Swap => matches!(self, Self::Anonymous),
            CapabilityOp::Phys => matches!(
                self,
                Self::Surface | Self::DeviceAllocation | Self::Anonymous | Self::Frame
            ),
            CapabilityOp::Grow => matches!(self, Self::Anonymous),
        }
    }
}

/// 类型化操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityOp {
    /// 建立 Region 映射。
    Map,
    /// 提交文档修订。
    Commit,
    /// 设置 scanout。
    Scanout,
    /// 映射为可执行。
    Execute,
    /// 固定驻留。
    Pin,
    /// 导出给其他进程或设备。
    Export,
    /// 压缩/交换匿名页。
    Swap,
    /// 查询物理区间。
    Phys,
    /// 增长/截断。
    Grow,
}

/// 一份类型化能力。
#[derive(Clone)]
pub struct Capability {
    id: ObjectId,
    kind: CapabilityKind,
    size: u64,
    page_size: usize,
    provider: Option<ProviderId>,
}

impl Capability {
    /// 创建一份类型化能力。
    pub fn new(
        kind: CapabilityKind,
        size: u64,
        page_size: usize,
        provider: Option<ProviderId>,
    ) -> Result<Self, HmpError> {
        Self::with_id(
            ObjectId::new(NEXT_CAPABILITY_ID.fetch_add(1, Ordering::Relaxed)),
            kind,
            size,
            page_size,
            provider,
        )
    }

    /// 以指定标识创建能力，供确定性测试使用。
    pub fn with_id(
        id: ObjectId,
        kind: CapabilityKind,
        size: u64,
        page_size: usize,
        provider: Option<ProviderId>,
    ) -> Result<Self, HmpError> {
        if size == 0
            || page_size == 0
            || !page_size.is_power_of_two()
            || !size.is_multiple_of(page_size as u64)
        {
            return Err(HmpError::InvalidSize);
        }
        Ok(Self {
            id,
            kind,
            size,
            page_size,
            provider,
        })
    }

    /// 返回能力标识。
    pub const fn id(&self) -> ObjectId {
        self.id
    }

    /// 返回能力类型。
    pub const fn kind(&self) -> CapabilityKind {
        self.kind
    }

    /// 返回字节长度。
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// 返回页大小。
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// 返回 Provider。
    pub const fn provider(&self) -> Option<ProviderId> {
        self.provider
    }

    /// 判断是否支持一项类型化操作。
    pub fn allows(&self, operation: CapabilityOp) -> bool {
        self.kind.supports(operation)
    }

    /// 要求一项类型化操作，类型不匹配时返回 `WrongCapability`。
    pub fn require(&self, operation: CapabilityOp) -> Result<(), HmpError> {
        if self.allows(operation) {
            Ok(())
        } else {
            Err(HmpError::WrongCapability)
        }
    }
}

/// Mappable 是 Region 可引用的统一能力接口。
pub trait Mappable {
    /// 返回能力描述。
    fn capability(&self) -> &Capability;

    /// 返回字节长度。
    fn size(&self) -> u64 {
        self.capability().size()
    }

    /// 返回页大小。
    fn page_size(&self) -> usize {
        self.capability().page_size()
    }

    /// 返回 Provider。
    fn provider(&self) -> Option<ProviderId> {
        self.capability().provider()
    }
}

impl Mappable for Capability {
    fn capability(&self) -> &Capability {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_specific_operations_are_enforced() {
        assert!(CapabilityKind::Revision.supports(CapabilityOp::Commit));
        assert!(CapabilityKind::Surface.supports(CapabilityOp::Scanout));
        assert!(CapabilityKind::TaskImage.supports(CapabilityOp::Execute));
        assert!(CapabilityKind::Frame.supports(CapabilityOp::Execute));
        assert!(CapabilityKind::DeviceAllocation.supports(CapabilityOp::Pin));
        assert!(CapabilityKind::Anonymous.supports(CapabilityOp::Swap));
        assert!(CapabilityKind::Frame.supports(CapabilityOp::Phys));
        assert!(CapabilityKind::Surface.supports(CapabilityOp::Phys));
        assert!(CapabilityKind::DeviceAllocation.supports(CapabilityOp::Phys));
        assert!(CapabilityKind::Anonymous.supports(CapabilityOp::Phys));

        assert!(!CapabilityKind::Anonymous.supports(CapabilityOp::Commit));
        assert!(!CapabilityKind::Revision.supports(CapabilityOp::Scanout));
        assert!(!CapabilityKind::Anonymous.supports(CapabilityOp::Execute));
        assert!(!CapabilityKind::Revision.supports(CapabilityOp::Phys));
        assert!(!CapabilityKind::TaskImage.supports(CapabilityOp::Phys));
    }

    #[test]
    fn capability_validates_layout_and_operations() {
        let capability =
            Capability::new(CapabilityKind::Revision, 0x2000, 4096, None).expect("能力");
        assert!(capability.allows(CapabilityOp::Map));
        assert_eq!(
            capability.require(CapabilityOp::Scanout).err(),
            Some(HmpError::WrongCapability)
        );
        assert_eq!(
            Capability::new(CapabilityKind::Surface, 0x1000, 4096, None)
                .expect("能力")
                .size(),
            0x1000
        );
        assert_eq!(
            Capability::new(CapabilityKind::Surface, 0x1000, 4096, None).err(),
            None
        );
    }

    #[test]
    fn rejects_unaligned_capability_size() {
        assert_eq!(
            Capability::new(CapabilityKind::Anonymous, 0x1001, 4096, None).err(),
            Some(HmpError::InvalidSize)
        );
    }

    #[test]
    fn mappable_exposes_capability_layout() {
        let capability =
            Capability::new(CapabilityKind::Surface, 0x2000, 4096, None).expect("能力");
        assert_eq!(capability.size(), 0x2000);
        assert_eq!(capability.page_size(), 4096);
        assert_eq!(capability.provider(), None);
    }
}
