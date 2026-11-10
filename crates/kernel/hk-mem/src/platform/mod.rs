//! 平台内存设施：DMA 分配器与软件 IOMMU。

pub mod dma;
pub mod iommu;

pub use dma::DmaAllocator;
pub use iommu::SoftwareIommu;
