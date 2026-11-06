//! 平台内存设施：DMA 池与软件 IOMMU。

pub mod dma;
pub mod iommu;

pub use dma::DmaPool;
pub use iommu::SoftwareIommu;
