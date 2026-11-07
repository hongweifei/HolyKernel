//! Provider 契约：类型化能力的缺页、回写与驻留服务。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_handle::{ObjectId, Rights};

use super::{CapabilityKind, FrameId, HmpError};

static NEXT_PROVIDER_ID: AtomicU64 = AtomicU64::new(1);

/// Provider 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(u64);

impl ProviderId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Provider 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// 文档修订。
    Revision,
    /// 显示表面。
    Surface,
    /// 任务镜像。
    Image,
    /// 设备/显存。
    Device,
    /// 匿名内存。
    Anonymous,
}

impl ProviderKind {
    /// 判断该 Provider 是否服务给定能力类型。
    pub const fn supports(self, capability: CapabilityKind) -> bool {
        match self {
            Self::Revision => matches!(capability, CapabilityKind::Revision),
            Self::Surface => matches!(capability, CapabilityKind::Surface),
            Self::Image => matches!(capability, CapabilityKind::TaskImage),
            Self::Device => matches!(capability, CapabilityKind::DeviceAllocation),
            Self::Anonymous => matches!(capability, CapabilityKind::Anonymous),
        }
    }
}

/// 缺页请求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRequest {
    /// 能力标识。
    pub capability: ObjectId,
    /// 能力内偏移。
    pub offset: u64,
    /// 请求长度。
    pub len: usize,
    /// 请求的访问权限。
    pub rights: Rights,
}

impl FaultRequest {
    /// 构造缺页请求。
    pub const fn new(capability: ObjectId, offset: u64, len: usize, rights: Rights) -> Self {
        Self {
            capability,
            offset,
            len,
            rights,
        }
    }
}

/// 缺页响应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultReply {
    /// 提供物理页。
    Provide {
        /// 物理页。
        frame: FrameId,
        /// 能力内偏移。
        offset: u64,
        /// 提供长度。
        len: usize,
    },
    /// 返回错误。
    Error(HmpError),
}

/// Provider 服务接口。
pub trait Provider {
    /// 返回 Provider 类型。
    fn kind(&self) -> ProviderKind;

    /// 判断该 Provider 是否服务给定能力类型。
    fn supports(&self, capability: CapabilityKind) -> bool {
        self.kind().supports(capability)
    }

    /// 处理缺页请求；默认返回 `ProviderMissing`。
    fn fault(&self, _request: FaultRequest) -> FaultReply {
        FaultReply::Error(HmpError::ProviderMissing)
    }
}

/// 分配一个 Provider 标识。
pub fn next_provider_id() -> ProviderId {
    ProviderId::new(NEXT_PROVIDER_ID.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_contract_matches_capability_kind() {
        assert!(ProviderKind::Revision.supports(CapabilityKind::Revision));
        assert!(ProviderKind::Surface.supports(CapabilityKind::Surface));
        assert!(ProviderKind::Image.supports(CapabilityKind::TaskImage));
        assert!(ProviderKind::Device.supports(CapabilityKind::DeviceAllocation));
        assert!(ProviderKind::Anonymous.supports(CapabilityKind::Anonymous));
        assert!(!ProviderKind::Anonymous.supports(CapabilityKind::Surface));
    }

    #[test]
    fn fault_messages_carry_capability_and_offset() {
        let request = FaultRequest::new(ObjectId::new(7), 0x2000, 0x1000, Rights::READ);
        assert_eq!(request.capability.get(), 7);
        assert_eq!(request.offset, 0x2000);
        let reply = FaultReply::Error(HmpError::ProviderMissing);
        assert_eq!(reply, FaultReply::Error(HmpError::ProviderMissing));
    }

    #[test]
    fn provider_defaults_report_supported_types_and_missing_faults() {
        struct AnonymousProvider;

        impl Provider for AnonymousProvider {
            fn kind(&self) -> ProviderKind {
                ProviderKind::Anonymous
            }
        }

        let provider = AnonymousProvider;
        assert!(provider.supports(CapabilityKind::Anonymous));
        assert!(!provider.supports(CapabilityKind::Surface));
        let request = FaultRequest::new(ObjectId::new(1), 0, 0x1000, Rights::READ);
        assert_eq!(
            provider.fault(request),
            FaultReply::Error(HmpError::ProviderMissing)
        );
    }
}
