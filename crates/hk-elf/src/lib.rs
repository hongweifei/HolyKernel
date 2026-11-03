//! 用户镜像 ELF 装载。
//!
//! 遍历程序头中的可加载段，把文件内容与内存长度交给 [`PageSink`]；页分配与
//! 页表映射由平台实现。

#![no_std]

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
