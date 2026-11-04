//! HolyFS v1：原生文件系统。
//!
//! 单文件按连续 extent 存储，写入走 COW（新 extent 提交后再释放旧 extent），
//! 顺序追加走连续快路径。元数据块与 inode 带 CRC32C，双超级块按 generation
//! 选择最新有效版本；整文档读取以一次后端调用完成。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::fmt;

use hk_block::{BlockDevice, BlockError};
use hk_vfs::{Backend, DirEntry, FileType, Metadata, NodeId, VfsError};

/// 文件系统块大小。
pub const BLOCK_SIZE: usize = 4096;
/// 超级块魔数。
const MAGIC: u64 = 0x484F_4C59_4653_3031; // "HOLYFS01"
/// 格式版本。
const VERSION: u32 = 1;
/// inode 记录魔数。
const INODE_MAGIC: u32 = 0x4849_4E4F;
/// 超级块 A 的块号。
const SUPER_A: u64 = 0;
/// 超级块 B 的块号。
const SUPER_B: u64 = 1;
/// 块位图块号。
const BLOCK_BITMAP: u64 = 2;
/// inode 位图块号。
const INODE_BITMAP: u64 = 3;
/// inode 表起始块号。
const INODE_TABLE_START: u64 = 4;
/// 最大 inode 数。
const MAX_INODES: u32 = 512;
/// inode 记录大小。
const INODE_SIZE: usize = 128;
/// inode 表占用块数。
const INODE_TABLE_BLOCKS: u64 = (MAX_INODES as usize * INODE_SIZE / BLOCK_SIZE) as u64;
/// 数据区起始块号。
const DATA_START: u64 = INODE_TABLE_START + INODE_TABLE_BLOCKS;
/// 根 inode 编号。
const ROOT_INODE: u64 = 1;
/// 单次块 I/O 的最大字节数，保证 IPC 消息不超过 64 KiB。
const IO_CHUNK: usize = 60 * 1024;

/// HolyFS 错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolyError {
    /// 设备 I/O 错误。
    Io,
    /// 超级块、inode 或目录结构无效。
    Corrupt,
    /// 空间不足。
    NoSpace,
    /// 节点不存在。
    NotFound,
    /// 不是目录。
    NotDirectory,
    /// 是目录。
    IsDirectory,
    /// 名称已存在。
    Exists,
    /// 参数无效。
    Invalid,
    /// 不支持的操作。
    Unsupported,
}

impl fmt::Display for HolyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "HolyFS I/O 错误",
            Self::Corrupt => "HolyFS 元数据损坏",
            Self::NoSpace => "HolyFS 空间不足",
            Self::NotFound => "HolyFS 节点不存在",
            Self::NotDirectory => "HolyFS 不是目录",
            Self::IsDirectory => "HolyFS 是目录",
            Self::Exists => "HolyFS 名称已存在",
            Self::Invalid => "HolyFS 参数无效",
            Self::Unsupported => "HolyFS 不支持的操作",
        })
    }
}

impl core::error::Error for HolyError {}

fn map_vfs(error: HolyError) -> VfsError {
    match error {
        HolyError::NotFound => VfsError::NotFound,
        HolyError::NotDirectory => VfsError::NotDirectory,
        HolyError::IsDirectory => VfsError::IsDirectory,
        HolyError::Exists => VfsError::AlreadyExists,
        HolyError::NoSpace => VfsError::Backend,
        HolyError::Invalid | HolyError::Corrupt | HolyError::Io | HolyError::Unsupported => {
            VfsError::Backend
        }
    }
}

fn map_io(_error: BlockError) -> HolyError {
    HolyError::Io
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Superblock {
    generation: u64,
    total_blocks: u64,
    root_inode: u32,
    slot_b: bool,
}

impl Superblock {
    fn encode(&self) -> [u8; 64] {
        let mut bytes = [0_u8; 64];
        bytes[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.generation.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.total_blocks.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.root_inode.to_le_bytes());
        bytes[36..40].copy_from_slice(&(DATA_START as u32).to_le_bytes());
        let checksum = crc32c(&bytes[..64]);
        bytes[48..52].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 64 {
            return None;
        }
        let magic = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let version = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let block_size = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
        if magic != MAGIC || version != VERSION || block_size as usize != BLOCK_SIZE {
            return None;
        }
        let mut copy = [0_u8; 64];
        copy.copy_from_slice(&bytes[..64]);
        let stored = u32::from_le_bytes(copy[48..52].try_into().ok()?);
        copy[48..52].fill(0);
        if crc32c(&copy) != stored {
            return None;
        }
        Some(Self {
            generation: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            total_blocks: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
            root_inode: u32::from_le_bytes(bytes[32..36].try_into().ok()?),
            slot_b: false,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Inode {
    id: u64,
    kind: FileType,
    links: u16,
    size: u64,
    extent_start: u64,
    extent_blocks: u32,
}

impl Inode {
    fn encode(&self) -> [u8; INODE_SIZE] {
        let mut bytes = [0_u8; INODE_SIZE];
        bytes[0..4].copy_from_slice(&INODE_MAGIC.to_le_bytes());
        bytes[4] = match self.kind {
            FileType::File => 0,
            FileType::Directory => 1,
        };
        bytes[5..7].copy_from_slice(&self.links.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.size.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.extent_start.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.extent_blocks.to_le_bytes());
        let checksum = crc32c(&bytes);
        bytes[36..40].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn decode(id: u64, bytes: &[u8]) -> Option<Self> {
        if bytes.len() < INODE_SIZE {
            return None;
        }
        let magic = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        if magic != INODE_MAGIC {
            return None;
        }
        let mut copy = [0_u8; INODE_SIZE];
        copy.copy_from_slice(&bytes[..INODE_SIZE]);
        let stored = u32::from_le_bytes(copy[36..40].try_into().ok()?);
        copy[36..40].fill(0);
        if crc32c(&copy) != stored {
            return None;
        }
        let kind = match bytes[4] {
            0 => FileType::File,
            1 => FileType::Directory,
            _ => return None,
        };
        Some(Self {
            id,
            kind,
            links: u16::from_le_bytes(bytes[5..7].try_into().ok()?),
            size: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            extent_start: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            extent_blocks: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
        })
    }
}

#[derive(Clone)]
struct DirItem {
    inode: u32,
    name: String,
    kind: FileType,
}

/// HolyFS 后端。
pub struct HolyFs<D: BlockDevice> {
    device: RefCell<D>,
    superblock: RefCell<Superblock>,
    block_bitmap: RefCell<Vec<u8>>,
    inode_bitmap: RefCell<Vec<u8>>,
    inode_cache: RefCell<BTreeMap<u64, Inode>>,
    next_hint: Cell<u64>,
}

impl<D: BlockDevice> HolyFs<D> {
    /// 挂载 HolyFS，选择 generation 最高的有效超级块。
    pub fn mount(device: D) -> Result<Self, HolyError> {
        let fs = Self {
            device: RefCell::new(device),
            superblock: RefCell::new(Superblock {
                generation: 0,
                total_blocks: 0,
                root_inode: ROOT_INODE as u32,
                slot_b: false,
            }),
            block_bitmap: RefCell::new(vec![0_u8; BLOCK_SIZE]),
            inode_bitmap: RefCell::new(vec![0_u8; BLOCK_SIZE]),
            inode_cache: RefCell::new(BTreeMap::new()),
            next_hint: Cell::new(DATA_START),
        };
        let mut block = [0_u8; BLOCK_SIZE];
        fs.read_block(SUPER_A, &mut block)?;
        let a = Superblock::decode(&block);
        fs.read_block(SUPER_B, &mut block)?;
        let mut b = Superblock::decode(&block);
        if let Some(superblock) = b.as_mut() {
            superblock.slot_b = true;
        }
        let chosen = match (a, b) {
            (Some(a), Some(b)) => {
                if b.generation > a.generation {
                    b
                } else {
                    a
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return Err(HolyError::Corrupt),
        };
        let capacity = fs.device.borrow().capacity_blocks();
        let sectors = (BLOCK_SIZE / fs.device.borrow().block_size()) as u64;
        if chosen.total_blocks == 0 || chosen.total_blocks > capacity / sectors {
            return Err(HolyError::Corrupt);
        }

        let mut block_bitmap = vec![0_u8; BLOCK_SIZE];
        fs.read_block(BLOCK_BITMAP, &mut block_bitmap)?;
        let mut inode_bitmap = vec![0_u8; BLOCK_SIZE];
        fs.read_block(INODE_BITMAP, &mut inode_bitmap)?;
        *fs.block_bitmap.borrow_mut() = block_bitmap;
        *fs.inode_bitmap.borrow_mut() = inode_bitmap;
        *fs.superblock.borrow_mut() = chosen;
        Ok(fs)
    }

    fn read_block(&self, block: u64, buffer: &mut [u8]) -> Result<(), HolyError> {
        let device = self.device.borrow();
        let sectors = BLOCK_SIZE / device.block_size();
        device
            .read_blocks(block * sectors as u64, &mut buffer[..BLOCK_SIZE])
            .map(|_| ())
            .map_err(map_io)
    }

    fn write_block(&self, block: u64, buffer: &[u8]) -> Result<(), HolyError> {
        let mut device = self.device.borrow_mut();
        let sectors = BLOCK_SIZE / device.block_size();
        device
            .write_blocks(block * sectors as u64, &buffer[..BLOCK_SIZE])
            .map(|_| ())
            .map_err(map_io)
    }

    fn read_bytes(&self, offset: u64, buffer: &mut [u8]) -> Result<(), HolyError> {
        let device = self.device.borrow();
        let block_size = device.block_size();
        let sector = offset / block_size as u64;
        let within = (offset % block_size as u64) as usize;
        if within == 0 && buffer.len().is_multiple_of(block_size) {
            for (index, chunk) in buffer.chunks_mut(IO_CHUNK).enumerate() {
                device
                    .read_blocks(sector + (index * IO_CHUNK / block_size) as u64, chunk)
                    .map_err(map_io)?;
            }
            return Ok(());
        }
        drop(device);
        let mut block = [0_u8; BLOCK_SIZE];
        let mut done = 0;
        while done < buffer.len() {
            let position = offset + done as u64;
            let fs_block = position / BLOCK_SIZE as u64;
            let inside = (position % BLOCK_SIZE as u64) as usize;
            self.read_block(fs_block, &mut block)?;
            let take = (BLOCK_SIZE - inside).min(buffer.len() - done);
            buffer[done..done + take].copy_from_slice(&block[inside..inside + take]);
            done += take;
        }
        Ok(())
    }

    fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), HolyError> {
        let mut device = self.device.borrow_mut();
        let block_size = device.block_size();
        let sector = offset / block_size as u64;
        let within = (offset % block_size as u64) as usize;
        if within == 0 && data.len().is_multiple_of(block_size) {
            for (index, chunk) in data.chunks(IO_CHUNK).enumerate() {
                device
                    .write_blocks(sector + (index * IO_CHUNK / block_size) as u64, chunk)
                    .map_err(map_io)?;
            }
            return Ok(());
        }
        drop(device);
        let mut block = [0_u8; BLOCK_SIZE];
        let mut done = 0;
        while done < data.len() {
            let position = offset + done as u64;
            let fs_block = position / BLOCK_SIZE as u64;
            let inside = (position % BLOCK_SIZE as u64) as usize;
            let take = (BLOCK_SIZE - inside).min(data.len() - done);
            if take != BLOCK_SIZE {
                self.read_block(fs_block, &mut block)?;
            }
            block[inside..inside + take].copy_from_slice(&data[done..done + take]);
            self.write_block(fs_block, &block)?;
            done += take;
        }
        Ok(())
    }

    fn inode(&self, id: u64) -> Result<Inode, HolyError> {
        if id == 0 || id > MAX_INODES as u64 {
            return Err(HolyError::NotFound);
        }
        if let Some(inode) = self.inode_cache.borrow().get(&id) {
            return Ok(*inode);
        }
        let index = (id - 1) as usize;
        let byte = index * INODE_SIZE;
        let block = INODE_TABLE_START + (byte / BLOCK_SIZE) as u64;
        let offset = byte % BLOCK_SIZE;
        let mut buffer = [0_u8; BLOCK_SIZE];
        self.read_block(block, &mut buffer)?;
        let inode =
            Inode::decode(id, &buffer[offset..offset + INODE_SIZE]).ok_or(HolyError::Corrupt)?;
        self.inode_cache.borrow_mut().insert(id, inode);
        Ok(inode)
    }

    fn write_inode(&self, inode: &Inode) -> Result<(), HolyError> {
        let index = (inode.id - 1) as usize;
        let byte = index * INODE_SIZE;
        let block = INODE_TABLE_START + (byte / BLOCK_SIZE) as u64;
        let offset = byte % BLOCK_SIZE;
        let mut buffer = [0_u8; BLOCK_SIZE];
        self.read_block(block, &mut buffer)?;
        buffer[offset..offset + INODE_SIZE].copy_from_slice(&inode.encode());
        self.write_block(block, &buffer)?;
        self.inode_cache.borrow_mut().insert(inode.id, *inode);
        Ok(())
    }

    fn allocate_inode(&self) -> Result<u64, HolyError> {
        let mut bitmap = self.inode_bitmap.borrow_mut();
        for index in 0..MAX_INODES as usize {
            let byte = index / 8;
            let bit = 1 << (index % 8);
            if bitmap[byte] & bit == 0 {
                bitmap[byte] |= bit;
                let id = index as u64 + 1;
                let zero = Inode {
                    id,
                    kind: FileType::File,
                    links: 1,
                    size: 0,
                    extent_start: 0,
                    extent_blocks: 0,
                };
                drop(bitmap);
                self.write_inode(&zero)?;
                self.write_inode_bitmap()?;
                return Ok(id);
            }
        }
        Err(HolyError::NoSpace)
    }

    fn free_inode(&self, id: u64) -> Result<(), HolyError> {
        if id == 0 || id > MAX_INODES as u64 {
            return Err(HolyError::NotFound);
        }
        let index = (id - 1) as usize;
        self.inode_bitmap.borrow_mut()[index / 8] &= !(1 << (index % 8));
        self.write_inode_bitmap()
    }

    fn write_inode_bitmap(&self) -> Result<(), HolyError> {
        let bitmap = self.inode_bitmap.borrow();
        let mut block = [0_u8; BLOCK_SIZE];
        block.copy_from_slice(&bitmap);
        drop(bitmap);
        self.write_block(INODE_BITMAP, &block)
    }

    fn write_block_bitmap(&self) -> Result<(), HolyError> {
        let bitmap = self.block_bitmap.borrow();
        let mut block = [0_u8; BLOCK_SIZE];
        block.copy_from_slice(&bitmap);
        drop(bitmap);
        self.write_block(BLOCK_BITMAP, &block)
    }

    fn block_allocated(&self, block: u64) -> bool {
        let index = block as usize;
        self.block_bitmap.borrow()[index / 8] & (1 << (index % 8)) != 0
    }

    fn mark_block(&self, block: u64, allocated: bool) {
        let index = block as usize;
        let mask = 1 << (index % 8);
        let mut bitmap = self.block_bitmap.borrow_mut();
        if allocated {
            bitmap[index / 8] |= mask;
        } else {
            bitmap[index / 8] &= !mask;
        }
    }

    fn allocate_blocks(&self, count: u64) -> Result<u64, HolyError> {
        let total = self.superblock.borrow().total_blocks;
        if count == 0 {
            return Ok(0);
        }
        let mut start = self.next_hint.get().max(DATA_START);
        let mut run = 0;
        let mut candidate = start;
        while start + count <= total {
            if !self.block_allocated(start) {
                if run == 0 {
                    candidate = start;
                }
                run += 1;
                if run == count {
                    for block in candidate..candidate + count {
                        self.mark_block(block, true);
                    }
                    self.next_hint.set(candidate + count);
                    self.write_block_bitmap()?;
                    return Ok(candidate);
                }
            } else {
                run = 0;
            }
            start += 1;
        }
        // 从数据区起点再扫一次，跳过提示点之后的空间。
        run = 0;
        for block in DATA_START..self.next_hint.get().max(DATA_START).min(total) {
            if !self.block_allocated(block) {
                if run == 0 {
                    candidate = block;
                }
                run += 1;
                if run == count {
                    for block in candidate..candidate + count {
                        self.mark_block(block, true);
                    }
                    self.next_hint.set(candidate + count);
                    self.write_block_bitmap()?;
                    return Ok(candidate);
                }
            } else {
                run = 0;
            }
        }
        Err(HolyError::NoSpace)
    }

    fn free_blocks(&self, start: u64, count: u64) -> Result<(), HolyError> {
        if count == 0 {
            return Ok(());
        }
        for block in start..start + count {
            self.mark_block(block, false);
        }
        if start < self.next_hint.get() {
            self.next_hint.set(start);
        }
        self.write_block_bitmap()
    }

    fn entries(&self, inode: &Inode) -> Result<Vec<DirItem>, HolyError> {
        if inode.kind != FileType::Directory {
            return Err(HolyError::NotDirectory);
        }
        let mut block = [0_u8; BLOCK_SIZE];
        self.read_block(inode.extent_start, &mut block)?;
        let mut items = Vec::new();
        let mut offset = 0;
        while offset + 8 <= BLOCK_SIZE {
            let id = u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap());
            if id == 0 {
                break;
            }
            let name_len =
                u16::from_le_bytes(block[offset + 4..offset + 6].try_into().unwrap()) as usize;
            let kind = match block[offset + 6] {
                0 => FileType::File,
                _ => FileType::Directory,
            };
            if offset + 8 + name_len > BLOCK_SIZE {
                return Err(HolyError::Corrupt);
            }
            let name = core::str::from_utf8(&block[offset + 8..offset + 8 + name_len])
                .map_err(|_| HolyError::Corrupt)?;
            items.push(DirItem {
                inode: id,
                name: String::from(name),
                kind,
            });
            offset += (8 + name_len).next_multiple_of(4);
        }
        Ok(items)
    }

    fn write_entries(&self, inode: &Inode, items: &[DirItem]) -> Result<(), HolyError> {
        let mut block = [0_u8; BLOCK_SIZE];
        let mut offset = 0;
        for item in items {
            let name = item.name.as_bytes();
            let entry = (8 + name.len()).next_multiple_of(4);
            if offset + entry > BLOCK_SIZE {
                return Err(HolyError::NoSpace);
            }
            block[offset..offset + 4].copy_from_slice(&item.inode.to_le_bytes());
            block[offset + 4..offset + 6].copy_from_slice(&(name.len() as u16).to_le_bytes());
            block[offset + 6] = match item.kind {
                FileType::File => 0,
                FileType::Directory => 1,
            };
            block[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
            offset += entry;
        }
        self.write_block(inode.extent_start, &block)
    }

    fn lookup_inode(&self, parent: u64, name: &str) -> Result<u64, HolyError> {
        let inode = self.inode(parent)?;
        let items = self.entries(&inode)?;
        items
            .iter()
            .find(|item| item.name == name)
            .map(|item| item.inode as u64)
            .ok_or(HolyError::NotFound)
    }

    fn commit(&self) -> Result<(), HolyError> {
        let mut superblock = *self.superblock.borrow();
        superblock.generation = superblock.generation.saturating_add(1);
        let (slot, next_slot) = if superblock.slot_b {
            (SUPER_A, false)
        } else {
            (SUPER_B, true)
        };
        let mut buffer = [0_u8; BLOCK_SIZE];
        buffer[..64].copy_from_slice(&superblock.encode());
        self.write_block(slot, &buffer)?;
        superblock.slot_b = next_slot;
        *self.superblock.borrow_mut() = superblock;
        self.device.borrow_mut().flush().map_err(map_io)
    }

    fn rewrite_file(&self, inode: &mut Inode, data: &[u8]) -> Result<(), HolyError> {
        let blocks = data.len().div_ceil(BLOCK_SIZE) as u64;
        let old_start = inode.extent_start;
        let old_blocks = inode.extent_blocks as u64;
        let new_start = if blocks == 0 {
            0
        } else {
            self.allocate_blocks(blocks)?
        };
        if blocks > 0 {
            let mut padded = vec![0_u8; blocks as usize * BLOCK_SIZE];
            padded[..data.len()].copy_from_slice(data);
            self.write_bytes(new_start * BLOCK_SIZE as u64, &padded)?;
        }
        inode.size = data.len() as u64;
        inode.extent_start = new_start;
        inode.extent_blocks = blocks as u32;
        self.write_inode(inode)?;
        if old_blocks > 0 {
            self.free_blocks(old_start, old_blocks)?;
        }
        Ok(())
    }

    fn write_file(&self, inode: &mut Inode, offset: u64, data: &[u8]) -> Result<(), HolyError> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(HolyError::Invalid)?;
        let new_size = inode.size.max(end);
        let needed_blocks = new_size.div_ceil(BLOCK_SIZE as u64);
        let append = offset == inode.size;
        let contiguous = inode.extent_blocks as u64 == inode.size.div_ceil(BLOCK_SIZE as u64);
        if append
            && contiguous
            && needed_blocks >= inode.extent_blocks as u64
            && (inode.extent_blocks == 0
                || self.blocks_free(
                    inode.extent_start + inode.extent_blocks as u64,
                    needed_blocks - inode.extent_blocks as u64,
                ))
        {
            if inode.extent_blocks == 0 {
                let start = self.allocate_blocks(needed_blocks)?;
                inode.extent_start = start;
            } else {
                for block in inode.extent_start + inode.extent_blocks as u64
                    ..inode.extent_start + needed_blocks
                {
                    self.mark_block(block, true);
                }
                self.write_block_bitmap()?;
            }
            self.write_bytes(inode.extent_start * BLOCK_SIZE as u64 + offset, data)?;
            inode.size = new_size;
            inode.extent_blocks = needed_blocks as u32;
            self.write_inode(inode)?;
            return Ok(());
        }

        let mut content = vec![0_u8; new_size as usize];
        if inode.size > 0 {
            self.read_bytes(
                inode.extent_start * BLOCK_SIZE as u64,
                &mut content[..inode.size as usize],
            )?;
        }
        content[offset as usize..end as usize].copy_from_slice(data);
        self.rewrite_file(inode, &content)
    }

    fn blocks_free(&self, start: u64, count: u64) -> bool {
        if count == 0 {
            return true;
        }
        let total = self.superblock.borrow().total_blocks;
        if start + count > total {
            return false;
        }
        (start..start + count).all(|block| !self.block_allocated(block))
    }
}

impl<D: BlockDevice> Backend for HolyFs<D> {
    fn root(&self) -> NodeId {
        ROOT_INODE
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        self.lookup_inode(parent, name).map_err(map_vfs)
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        let inode = self.inode(node).map_err(map_vfs)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
        })
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let inode = self.inode(node).map_err(map_vfs)?;
        if inode.kind == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }
        if offset >= inode.size || buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min((inode.size - offset) as usize);
        self.read_bytes(
            inode.extent_start * BLOCK_SIZE as u64 + offset,
            &mut buffer[..length],
        )
        .map_err(map_vfs)?;
        Ok(length)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let mut inode = self.inode(node).map_err(map_vfs)?;
        if inode.kind == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }
        self.write_file(&mut inode, offset, data).map_err(map_vfs)?;
        Ok(data.len())
    }

    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError> {
        if kind != FileType::File {
            return Err(VfsError::ReadOnly);
        }
        let parent_inode = self.inode(parent).map_err(map_vfs)?;
        if parent_inode.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        if self.lookup_inode(parent, name).is_ok() {
            return Err(VfsError::AlreadyExists);
        }
        let id = self.allocate_inode().map_err(map_vfs)?;
        let mut items = self.entries(&parent_inode).map_err(map_vfs)?;
        items.push(DirItem {
            inode: id as u32,
            name: String::from(name),
            kind: FileType::File,
        });
        self.write_entries(&parent_inode, &items).map_err(map_vfs)?;
        Ok(id)
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        let inode = self.inode(node).map_err(map_vfs)?;
        let items = self.entries(&inode).map_err(map_vfs)?;
        Ok(items
            .into_iter()
            .filter(|item| item.name != "." && item.name != "..")
            .map(|item| DirEntry {
                name: item.name,
                kind: item.kind,
            })
            .collect())
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        let mut inode = self.inode(node).map_err(map_vfs)?;
        if inode.kind == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }
        if size == 0 {
            let start = inode.extent_start;
            let blocks = inode.extent_blocks as u64;
            inode.size = 0;
            inode.extent_start = 0;
            inode.extent_blocks = 0;
            self.write_inode(&inode).map_err(map_vfs)?;
            self.free_blocks(start, blocks).map_err(map_vfs)?;
            return Ok(());
        }
        let mut content = vec![0_u8; size as usize];
        let copy = inode.size.min(size) as usize;
        if copy > 0 {
            self.read_bytes(inode.extent_start * BLOCK_SIZE as u64, &mut content[..copy])
                .map_err(map_vfs)?;
        }
        self.rewrite_file(&mut inode, &content).map_err(map_vfs)
    }

    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        let parent_inode = self.inode(parent).map_err(map_vfs)?;
        let mut items = self.entries(&parent_inode).map_err(map_vfs)?;
        let Some(index) = items.iter().position(|item| item.name == name) else {
            return Err(VfsError::NotFound);
        };
        let item = items.remove(index);
        let inode = self.inode(item.inode as u64).map_err(map_vfs)?;
        if inode.kind == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }
        self.write_entries(&parent_inode, &items).map_err(map_vfs)?;
        self.free_blocks(inode.extent_start, inode.extent_blocks as u64)
            .map_err(map_vfs)?;
        self.free_inode(inode.id).map_err(map_vfs)?;
        Ok(inode.id)
    }

    fn sync(&mut self) -> Result<(), VfsError> {
        self.commit().map_err(map_vfs)
    }
}

/// 格式化块设备为 HolyFS。
pub fn format<D: BlockDevice>(device: D) -> Result<D, HolyError> {
    {
        let device_ref = &device;
        let capacity = device_ref.capacity_blocks();
        let block_size = device_ref.block_size();
        if block_size == 0
            || !BLOCK_SIZE.is_multiple_of(block_size)
            || capacity / (BLOCK_SIZE / block_size) as u64 <= DATA_START
        {
            return Err(HolyError::NoSpace);
        }
    }
    let fs = HolyFs {
        device: RefCell::new(device),
        superblock: RefCell::new(Superblock {
            generation: 1,
            total_blocks: 0,
            root_inode: ROOT_INODE as u32,
            slot_b: false,
        }),
        block_bitmap: RefCell::new(vec![0_u8; BLOCK_SIZE]),
        inode_bitmap: RefCell::new(vec![0_u8; BLOCK_SIZE]),
        inode_cache: RefCell::new(BTreeMap::new()),
        next_hint: Cell::new(DATA_START + 1),
    };
    let capacity = fs.device.borrow().capacity_blocks();
    let total = capacity / (BLOCK_SIZE / fs.device.borrow().block_size()) as u64;
    {
        let mut superblock = fs.superblock.borrow_mut();
        superblock.total_blocks = total;
    }
    let zero = [0_u8; BLOCK_SIZE];
    for block in [SUPER_A, SUPER_B, BLOCK_BITMAP, INODE_BITMAP] {
        fs.write_block(block, &zero)?;
    }
    for block in INODE_TABLE_START..DATA_START {
        fs.write_block(block, &zero)?;
    }
    let root_data = DATA_START;
    fs.write_block(root_data, &zero)?;

    // 元数据块与根目录块已占用。
    for block in 0..=root_data {
        fs.mark_block(block, true);
    }
    fs.write_block_bitmap()?;
    fs.inode_bitmap.borrow_mut()[0] = 1;
    fs.write_inode_bitmap()?;

    let root = Inode {
        id: ROOT_INODE,
        kind: FileType::Directory,
        links: 2,
        size: BLOCK_SIZE as u64,
        extent_start: root_data,
        extent_blocks: 1,
    };
    fs.write_inode(&root)?;
    let items = [
        DirItem {
            inode: ROOT_INODE as u32,
            name: String::from("."),
            kind: FileType::Directory,
        },
        DirItem {
            inode: ROOT_INODE as u32,
            name: String::from(".."),
            kind: FileType::Directory,
        },
    ];
    fs.write_entries(&root, &items)?;
    fs.commit()?;
    Ok(fs.device.into_inner())
}

/// CRC32C（Castagnoli）校验。
fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use hk_block::{MemoryDisk, SECTOR_SIZE};

    const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

    fn formatted() -> MemoryDisk {
        format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化")
    }

    #[test]
    fn checksums_known_vector() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn creates_writes_and_reads_files() {
        let mut fs = HolyFs::mount(formatted()).expect("挂载");
        let root = fs.root();
        let node = fs.create(root, "hello", FileType::File).expect("创建");
        assert_eq!(fs.write(node, 0, b"holyfs").expect("写"), 6);
        let mut buffer = [0_u8; 16];
        assert_eq!(fs.read(node, 0, &mut buffer).expect("读"), 6);
        assert_eq!(&buffer[..6], b"holyfs");
        assert_eq!(fs.metadata(node).expect("元数据").size, 6);
        let entries = fs.readdir(root).expect("目录");
        assert!(entries.iter().any(|entry| entry.name == "hello"));
    }

    #[test]
    fn appends_and_reads_large_file() {
        let mut fs = HolyFs::mount(formatted()).expect("挂载");
        let root = fs.root();
        let node = fs.create(root, "large", FileType::File).expect("创建");
        let chunk = [0x5A_u8; 60 * 1024];
        for index in 0..17 {
            fs.write(node, index * chunk.len() as u64, &chunk)
                .expect("追加");
        }
        let size = fs.metadata(node).expect("元数据").size;
        assert_eq!(size, 17 * chunk.len() as u64);
        let mut buffer = [0_u8; 4096];
        let read = fs.read(node, size - 4096, &mut buffer).expect("读");
        assert_eq!(read, 4096);
        assert_eq!(buffer, [0x5A; 4096]);
    }

    #[test]
    fn truncates_and_unlinks() {
        let mut fs = HolyFs::mount(formatted()).expect("挂载");
        let root = fs.root();
        let node = fs.create(root, "file", FileType::File).expect("创建");
        fs.write(node, 0, b"0123456789").expect("写");
        fs.truncate(node, 4).expect("截断");
        assert_eq!(fs.metadata(node).expect("元数据").size, 4);
        assert_eq!(fs.unlink(root, "file").expect("删除"), node);
        assert_eq!(fs.lookup(root, "file"), Err(VfsError::NotFound));
    }

    struct Shared(Rc<RefCell<MemoryDisk>>);

    impl BlockDevice for Shared {
        fn block_size(&self) -> usize {
            SECTOR_SIZE
        }

        fn capacity_blocks(&self) -> u64 {
            self.0.borrow().capacity_blocks()
        }

        fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
            self.0.borrow().read_blocks(block, buffer)
        }

        fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
            self.0.borrow_mut().write_blocks(block, data)
        }

        fn flush(&mut self) -> Result<(), BlockError> {
            Ok(())
        }
    }

    #[test]
    fn persists_across_remount() {
        let disk = Rc::new(RefCell::new(formatted()));
        {
            let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
            let root = fs.root();
            let node = fs.create(root, "doc", FileType::File).expect("创建");
            fs.write(node, 0, b"HolyFS document").expect("写");
            fs.sync().expect("提交");
        }
        let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
        let root = fs.root();
        let node = fs.lookup(root, "doc").expect("查找");
        let mut buffer = [0_u8; 32];
        let read = fs.read(node, 0, &mut buffer).expect("读");
        assert_eq!(&buffer[..read], b"HolyFS document");
    }
}
