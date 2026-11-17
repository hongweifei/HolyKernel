//! 用户镜像装载：原生 HKX 段、用户栈与 Arena。
//!
//! 页的分配与映射由平台的 [`UserMemory`] 提供；本模块只负责段解析与布局。

use alloc::vec::Vec;
use core::cmp::min;

use hk_hal::{PAGE_SIZE, PhysAddr, UserMemory};
use hk_image::Image;
use hk_runtime::{ImageProviderSpec, ImageSource, UserRegionSpec};

/// 用户镜像装载错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// 镜像结构无效。
    Parse,
    /// 段映射失败。
    Map,
}

/// 段映射错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapError;

/// 段页映射接口。
pub trait PageSink {
    /// 把文件内容与零填充映射到 `virtual_address`。
    ///
    /// `file` 是段在镜像中的内容，`memory_size` 是段在内存中的总长度。
    fn map_segment(
        &mut self,
        virtual_address: u64,
        file: &[u8],
        file_offset: usize,
        memory_size: u64,
        flags: u32,
    ) -> Result<(), MapError>;
}

/// 装载全部段，返回入口地址。
pub fn load(image: &[u8], sink: &mut impl PageSink) -> Result<u64, ImageError> {
    let bytes = image;
    let image = Image::parse(bytes).ok_or(ImageError::Parse)?;
    let task = image.task().ok_or(ImageError::Parse)?;
    for region in image.regions() {
        let file_offset = region.file.as_ptr() as usize - bytes.as_ptr() as usize;
        sink.map_segment(
            region.virtual_address,
            region.file,
            file_offset,
            region.memory_size,
            region.flags,
        )
        .map_err(|MapError| ImageError::Map)?;
    }
    Ok(task.entry)
}

/// 把段直接映射进用户地址空间，返回入口地址。
///
/// 每页由 [`UserMemory::map_page`] 分配并清零，普通内存段写入文件内容，其余
/// 保持零填充。
pub fn load_into<M: UserMemory>(image: &[u8], memory: &mut M) -> Result<u64, ImageError> {
    load(
        image,
        &mut UserSink {
            memory,
            regions: None,
            source_base: None,
            lazy: false,
            provider_endpoint: None,
            module_index: 0,
        },
    )
}

/// 把段映射进用户地址空间，同时返回各段的 Region 描述。
pub fn load_into_with_regions<M: UserMemory>(
    image: &[u8],
    memory: &mut M,
) -> Result<(u64, Vec<UserRegionSpec>), ImageError> {
    let mut regions = Vec::new();
    let entry = load(
        image,
        &mut UserSink {
            memory,
            regions: Some(&mut regions),
            source_base: None,
            lazy: false,
            provider_endpoint: None,
            module_index: 0,
        },
    )?;
    Ok((entry, regions))
}

/// 装载镜像；可执行段可选择只登记来源，由内核在缺页时装入。
pub fn load_into_with_regions_from<M: UserMemory>(
    image: &[u8],
    memory: &mut M,
    source_base: Option<PhysAddr>,
    lazy: bool,
    provider_endpoint: Option<usize>,
    module_index: usize,
) -> Result<(u64, Vec<UserRegionSpec>), ImageError> {
    let mut regions = Vec::new();
    let entry = load(
        image,
        &mut UserSink {
            memory,
            regions: Some(&mut regions),
            source_base,
            lazy,
            provider_endpoint,
            module_index,
        },
    )?;
    Ok((entry, regions))
}

struct UserSink<'a, M> {
    memory: &'a mut M,
    regions: Option<&'a mut Vec<UserRegionSpec>>,
    source_base: Option<PhysAddr>,
    lazy: bool,
    provider_endpoint: Option<usize>,
    module_index: usize,
}

impl<M: UserMemory> PageSink for UserSink<'_, M> {
    fn map_segment(
        &mut self,
        virtual_address: u64,
        file: &[u8],
        file_offset: usize,
        memory_size: u64,
        flags: u32,
    ) -> Result<(), MapError> {
        let page_size = PAGE_SIZE as u64;
        let pages = memory_size.div_ceil(page_size);
        // ELF p_flags：1 = 可执行，2 = 可写，4 = 可读。W+X 段退化为只读可执行。
        let executable = flags & 1 != 0;
        let writable = flags & 2 != 0 && !executable;
        let lazy = self.lazy
            && executable
            && (self.source_base.is_some() || self.provider_endpoint.is_some());

        if !lazy {
            for page in 0..pages {
                let offset = page * page_size;
                {
                    let slice = self
                        .memory
                        .map_page(virtual_address + offset)
                        .ok_or(MapError)?;

                    let start = offset as usize;
                    if start < file.len() {
                        let length = min(PAGE_SIZE, file.len() - start);
                        slice[..length].copy_from_slice(&file[start..start + length]);
                    }
                }
                self.memory
                    .protect_page(virtual_address + offset, writable, executable);
            }
        }
        if pages > 0
            && let Some(regions) = self.regions.as_mut()
        {
            let source = if lazy && self.provider_endpoint.is_none() {
                Some(ImageSource {
                    phys: PhysAddr::new(
                        self.source_base
                            .map_or(0, |base| base.get())
                            .saturating_add(file_offset as u64),
                    ),
                    file_len: file.len(),
                })
            } else {
                None
            };
            let provider = lazy
                .then_some(self.provider_endpoint)
                .flatten()
                .map(|endpoint| ImageProviderSpec {
                    endpoint,
                    module: self.module_index,
                    file_offset,
                    file_len: file.len(),
                });
            regions.push(UserRegionSpec {
                virt: virtual_address,
                len: pages as usize * PAGE_SIZE,
                writable,
                executable,
                source,
                provider,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_image::{RegionData, TaskData, build};

    #[test]
    fn rejects_malformed_image() {
        let mut sink = NullSink;
        assert_eq!(load(b"not an image", &mut sink), Err(ImageError::Parse));
    }

    #[test]
    fn collects_region_specs_while_loading() {
        let image = build(
            TaskData {
                name: "test",
                role: 0,
                entry: 0x4000,
                stack_size: 0,
                arena_size: 0,
            },
            &[RegionData {
                virtual_address: 0x4000,
                data: &[1, 2, 3, 4],
                memory_size: 8,
                flags: 2,
            }],
            &[],
            &[],
            &[],
        );
        let mut memory = TestMemory::default();
        let (entry, regions) = load_into_with_regions(&image, &mut memory).expect("装载镜像");
        assert_eq!(entry, 0x4000);
        assert_eq!(
            regions,
            alloc::vec![UserRegionSpec {
                virt: 0x4000,
                len: PAGE_SIZE,
                writable: true,
                executable: false,
                source: None,
                provider: None,
            }]
        );
        assert_eq!(&memory.data[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn lazy_image_registers_source_without_mapping() {
        let image = build(
            TaskData {
                name: "test",
                role: 0,
                entry: 0x4000,
                stack_size: 0,
                arena_size: 0,
            },
            &[RegionData {
                virtual_address: 0x4000,
                data: &[0x90, 0x90, 0x90, 0x90],
                memory_size: 8,
                flags: 1,
            }],
            &[],
            &[],
            &[],
        );
        let mut memory = TestMemory::default();
        let (_, regions) = load_into_with_regions_from(
            &image,
            &mut memory,
            Some(PhysAddr::new(0x1000)),
            true,
            None,
            0,
        )
        .expect("装载镜像");
        assert!(memory.pages.is_empty(), "可执行段不应在装载期映射");
        assert_eq!(regions.len(), 1);
        assert!(regions[0].executable);
        let source = regions[0].source.expect("执行段来源");
        assert_eq!(source.file_len, 4);
        assert!(source.phys.get() >= 0x1000);
    }

    #[derive(Default)]
    struct TestMemory {
        data: Vec<u8>,
        pages: Vec<(u64, bool, bool)>,
    }

    impl UserMemory for TestMemory {
        fn map_page(&mut self, virtual_address: u64) -> Option<&mut [u8]> {
            self.pages.push((virtual_address, true, false));
            let start = self.data.len();
            self.data.resize(start + PAGE_SIZE, 0);
            Some(&mut self.data[start..])
        }

        fn protect_page(&mut self, virtual_address: u64, writable: bool, executable: bool) {
            if let Some(page) = self
                .pages
                .iter_mut()
                .rev()
                .find(|(address, _, _)| *address == virtual_address)
            {
                page.1 = writable;
                page.2 = executable;
            }
        }
    }

    #[test]
    fn loads_native_image() {
        let image = build(
            TaskData {
                name: "test",
                role: 0,
                entry: 0x4000,
                stack_size: 0,
                arena_size: 0,
            },
            &[RegionData {
                virtual_address: 0x4000,
                data: &[1, 2, 3, 4],
                memory_size: 8,
                flags: 0,
            }],
            &[],
            &[],
            &[],
        );
        let mut sink = CountingSink { segments: 0 };
        assert_eq!(load(&image, &mut sink), Ok(0x4000));
        assert_eq!(sink.segments, 1);
    }

    struct NullSink;

    impl PageSink for NullSink {
        fn map_segment(
            &mut self,
            _: u64,
            _: &[u8],
            _: usize,
            _: u64,
            _: u32,
        ) -> Result<(), MapError> {
            Ok(())
        }
    }

    struct CountingSink {
        segments: usize,
    }

    impl PageSink for CountingSink {
        fn map_segment(
            &mut self,
            _: u64,
            _: &[u8],
            _: usize,
            _: u64,
            _: u32,
        ) -> Result<(), MapError> {
            self.segments += 1;
            Ok(())
        }
    }
}
