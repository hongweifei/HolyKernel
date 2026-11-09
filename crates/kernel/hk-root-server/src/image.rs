//! 用户镜像装载：原生 HKX 段、用户栈与 Arena。
//!
//! 页的分配与映射由平台的 [`UserMemory`] 提供；本模块只负责段解析与布局。

use core::cmp::min;

use hk_hal::{PAGE_SIZE, UserMemory};
use hk_image::Image;

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
        memory_size: u64,
        flags: u32,
    ) -> Result<(), MapError>;
}

/// 装载全部段，返回入口地址。
pub fn load(image: &[u8], sink: &mut impl PageSink) -> Result<u64, ImageError> {
    let image = Image::parse(image).ok_or(ImageError::Parse)?;
    let task = image.task().ok_or(ImageError::Parse)?;
    for region in image.regions() {
        sink.map_segment(
            region.virtual_address,
            region.file,
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
    load(image, &mut UserSink { memory })
}

struct UserSink<'a, M> {
    memory: &'a mut M,
}

impl<M: UserMemory> PageSink for UserSink<'_, M> {
    fn map_segment(
        &mut self,
        virtual_address: u64,
        file: &[u8],
        memory_size: u64,
        flags: u32,
    ) -> Result<(), MapError> {
        let page_size = PAGE_SIZE as u64;
        let pages = memory_size.div_ceil(page_size);
        // ELF p_flags：1 = 可执行，2 = 可写，4 = 可读。W+X 段退化为只读可执行。
        let executable = flags & 1 != 0;
        let writable = flags & 2 != 0 && !executable;

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
        fn map_segment(&mut self, _: u64, _: &[u8], _: u64, _: u32) -> Result<(), MapError> {
            Ok(())
        }
    }

    struct CountingSink {
        segments: usize,
    }

    impl PageSink for CountingSink {
        fn map_segment(&mut self, _: u64, _: &[u8], _: u64, _: u32) -> Result<(), MapError> {
            self.segments += 1;
            Ok(())
        }
    }
}
