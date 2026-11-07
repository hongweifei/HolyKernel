//! x86_64 IOMMU 适配。
//!
//! 当前 QEMU 配置没有 VT-d；没有硬件 IOMMU 时使用 identity 后端，保留统一
//! 的域映射接口，后续接入 DMAR/VT-d 时替换该实例。

use hk_mem::SoftwareIommu;

/// 全局 IOMMU 后端。
pub static IOMMU: SoftwareIommu = SoftwareIommu::new();
