//! 用户镜像 ELF 装载。
//!
//! 遍历程序头中的可加载段，把文件内容与内存长度交给 [`PageSink`]；页分配与
//! 页表映射由平台实现。

#![no_std]

use core::cmp::min;

use hk_hal::{PAGE_SIZE, UserMemory};
use xmas_elf::ElfFile;
use xmas_elf::program::Type;

/// 用户镜像装载错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// ELF 结构无效。
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
    /// `file` 是段在文件中的内容，`memory_size` 是段在内存中的总长度。
    fn map_segment(
        &mut self,
        virtual_address: u64,
        file: &[u8],
        memory_size: u64,
    ) -> Result<(), MapError>;
}

/// 按程序头装载全部可加载段，返回入口地址。
pub fn load(elf: &[u8], sink: &mut impl PageSink) -> Result<u64, ElfError> {
    let file = ElfFile::new(elf).map_err(|_| ElfError::Parse)?;
    let entry = file.header.pt2.entry_point();

    for program in file.program_iter() {
        if program.get_type() != Ok(Type::Load) {
            continue;
        }

        let offset = program.offset() as usize;
        let file_size = program.file_size() as usize;
        let end = offset.checked_add(file_size).ok_or(ElfError::Parse)?;
        let data = elf.get(offset..end).ok_or(ElfError::Parse)?;

        sink.map_segment(program.virtual_addr(), data, program.mem_size())
            .map_err(|MapError| ElfError::Map)?;
    }

    Ok(entry)
}

/// 把可加载段直接映射进用户地址空间，返回入口地址。
///
/// 每页由 [`UserMemory::map_page`] 分配并清零，普通内存段写入文件内容，其余
/// 保持零填充。
pub fn load_into<M: UserMemory>(elf: &[u8], memory: &mut M) -> Result<u64, ElfError> {
    load(elf, &mut UserSink { memory })
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
    ) -> Result<(), MapError> {
        let page_size = PAGE_SIZE as u64;
        let pages = memory_size.div_ceil(page_size);

        for page in 0..pages {
            let offset = page * page_size;
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_image() {
        let mut sink = NullSink;
        assert_eq!(load(b"not an elf", &mut sink), Err(ElfError::Parse));
    }

    struct NullSink;

    impl PageSink for NullSink {
        fn map_segment(&mut self, _: u64, _: &[u8], _: u64) -> Result<(), MapError> {
            Ok(())
        }
    }
}
