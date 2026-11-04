//! 持久化 COW 元数据树。
//!
//! 目录、文档表与修订表都使用同一种按键排序的块树：叶节点保存有序键值，
//! 内部节点保存子节点与分隔键。每次提交构建一棵新树并切换根，旧树不原地修改。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::crc32c::crc32c;
use crate::error::HolyError;
use crate::store::Store;
use crate::superblock::BLOCK_SIZE;

/// 元数据树键值映射。
pub(crate) type TreeMap = BTreeMap<Vec<u8>, Vec<u8>>;

const NODE_MAGIC: u32 = 0x4842_5452;
const HEADER_SIZE: usize = 16;
const MAX_PAYLOAD: usize = BLOCK_SIZE - HEADER_SIZE;
const MAX_DEPTH: usize = 32;

pub(crate) fn store_tree<D: BlockDevice>(
    store: &mut Store<D>,
    map: &TreeMap,
) -> Result<u64, HolyError> {
    if map.is_empty() {
        return write_leaf(store, &[]).map(|(_, block)| block);
    }
    let mut level = Vec::new();
    let mut current = Vec::new();
    let mut size = 0;
    for (key, value) in map {
        let need = 4 + key.len() + value.len();
        if !current.is_empty() && size + need > MAX_PAYLOAD {
            level.push(write_leaf(store, &current)?);
            current.clear();
            size = 0;
        }
        if need > MAX_PAYLOAD {
            return Err(HolyError::Invalid);
        }
        current.push((key.clone(), value.clone()));
        size += need;
    }
    if !current.is_empty() {
        level.push(write_leaf(store, &current)?);
    }
    if level.len() == 1 {
        return Ok(level[0].1);
    }

    loop {
        let mut next = Vec::new();
        let mut current = Vec::new();
        let mut size = 0;
        for child in level {
            let need = 2 + child.0.len() + 8;
            if !current.is_empty() && size + need > MAX_PAYLOAD {
                next.push(write_internal(store, &current)?);
                current.clear();
                size = 0;
            }
            current.push(child);
            size += need;
        }
        if !current.is_empty() {
            next.push(write_internal(store, &current)?);
        }
        if next.len() == 1 {
            return Ok(next[0].1);
        }
        level = next;
    }
}

pub(crate) fn load_tree<D: BlockDevice>(store: &Store<D>, root: u64) -> Result<TreeMap, HolyError> {
    let mut map = TreeMap::new();
    load_into(store, root, &mut map, 0)?;
    Ok(map)
}

/// 收集一棵树占用的全部块，用于提交后回收旧元数据。
pub(crate) fn collect_blocks<D: BlockDevice>(
    store: &Store<D>,
    root: u64,
    blocks: &mut Vec<u64>,
) -> Result<(), HolyError> {
    collect_into(store, root, blocks, 0)
}

fn collect_into<D: BlockDevice>(
    store: &Store<D>,
    block: u64,
    blocks: &mut Vec<u64>,
    depth: usize,
) -> Result<(), HolyError> {
    if depth > MAX_DEPTH || block == 0 {
        return Err(HolyError::Corrupt);
    }
    let mut bytes = [0_u8; BLOCK_SIZE];
    store.read_block(block, &mut bytes)?;
    let magic = u32::from_le_bytes(bytes[0..4].try_into().map_err(|_| HolyError::Corrupt)?);
    let level = bytes[4];
    let count =
        u16::from_le_bytes(bytes[6..8].try_into().map_err(|_| HolyError::Corrupt)?) as usize;
    let used =
        u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| HolyError::Corrupt)?) as usize;
    let stored = u32::from_le_bytes(bytes[12..16].try_into().map_err(|_| HolyError::Corrupt)?);
    if magic != NODE_MAGIC || !(HEADER_SIZE..=BLOCK_SIZE).contains(&used) {
        return Err(HolyError::Corrupt);
    }
    let mut copy = bytes;
    copy[12..16].fill(0);
    if crc32c(&copy[..used]) != stored {
        return Err(HolyError::Corrupt);
    }
    blocks.push(block);
    if level == 0 {
        return Ok(());
    }
    let payload = &bytes[HEADER_SIZE..used];
    let mut offset = 0;
    for _ in 0..count {
        let key_len = read_u16(payload, &mut offset)? as usize;
        offset += key_len;
        let child = read_u64(payload, &mut offset)?;
        collect_into(store, child, blocks, depth + 1)?;
    }
    if offset != payload.len() {
        return Err(HolyError::Corrupt);
    }
    Ok(())
}

fn load_into<D: BlockDevice>(
    store: &Store<D>,
    block: u64,
    map: &mut TreeMap,
    depth: usize,
) -> Result<(), HolyError> {
    if depth > MAX_DEPTH {
        return Err(HolyError::Corrupt);
    }
    let mut bytes = [0_u8; BLOCK_SIZE];
    store.read_block(block, &mut bytes)?;
    let magic = u32::from_le_bytes(bytes[0..4].try_into().map_err(|_| HolyError::Corrupt)?);
    let level = bytes[4];
    let count =
        u16::from_le_bytes(bytes[6..8].try_into().map_err(|_| HolyError::Corrupt)?) as usize;
    let used =
        u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| HolyError::Corrupt)?) as usize;
    let stored = u32::from_le_bytes(bytes[12..16].try_into().map_err(|_| HolyError::Corrupt)?);
    if magic != NODE_MAGIC || !(HEADER_SIZE..=BLOCK_SIZE).contains(&used) {
        return Err(HolyError::Corrupt);
    }
    let mut copy = bytes;
    copy[12..16].fill(0);
    if crc32c(&copy[..used]) != stored {
        return Err(HolyError::Corrupt);
    }
    let payload = &bytes[HEADER_SIZE..used];
    if level == 0 {
        let mut offset = 0;
        for _ in 0..count {
            let key_len = read_u16(payload, &mut offset)? as usize;
            let value_len = read_u16(payload, &mut offset)? as usize;
            let key = payload
                .get(offset..offset + key_len)
                .ok_or(HolyError::Corrupt)?
                .to_vec();
            offset += key_len;
            let value = payload
                .get(offset..offset + value_len)
                .ok_or(HolyError::Corrupt)?
                .to_vec();
            offset += value_len;
            map.insert(key, value);
        }
        if offset != payload.len() {
            return Err(HolyError::Corrupt);
        }
    } else {
        let mut offset = 0;
        let mut children = Vec::with_capacity(count);
        for _ in 0..count {
            let key_len = read_u16(payload, &mut offset)? as usize;
            let _key = payload
                .get(offset..offset + key_len)
                .ok_or(HolyError::Corrupt)?;
            offset += key_len;
            let child = read_u64(payload, &mut offset)?;
            children.push(child);
        }
        if offset != payload.len() {
            return Err(HolyError::Corrupt);
        }
        for child in children {
            load_into(store, child, map, depth + 1)?;
        }
    }
    Ok(())
}

fn write_leaf<D: BlockDevice>(
    store: &mut Store<D>,
    entries: &[(Vec<u8>, Vec<u8>)],
) -> Result<(Vec<u8>, u64), HolyError> {
    let mut payload = Vec::new();
    for (key, value) in entries {
        if key.len() > u16::MAX as usize || value.len() > u16::MAX as usize {
            return Err(HolyError::Invalid);
        }
        payload.extend_from_slice(&(key.len() as u16).to_le_bytes());
        payload.extend_from_slice(&(value.len() as u16).to_le_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(value);
    }
    let block = write_node(store, 0, entries.len() as u16, &payload)?;
    let first = entries
        .first()
        .map(|entry| entry.0.clone())
        .unwrap_or_default();
    Ok((first, block))
}

fn write_internal<D: BlockDevice>(
    store: &mut Store<D>,
    children: &[(Vec<u8>, u64)],
) -> Result<(Vec<u8>, u64), HolyError> {
    let mut payload = Vec::new();
    for (key, child) in children {
        if key.len() > u16::MAX as usize {
            return Err(HolyError::Invalid);
        }
        payload.extend_from_slice(&(key.len() as u16).to_le_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(&child.to_le_bytes());
    }
    let block = write_node(store, 1, children.len() as u16, &payload)?;
    let first = children
        .first()
        .map(|child| child.0.clone())
        .unwrap_or_default();
    Ok((first, block))
}

fn write_node<D: BlockDevice>(
    store: &mut Store<D>,
    level: u8,
    count: u16,
    payload: &[u8],
) -> Result<u64, HolyError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(HolyError::Invalid);
    }
    let used = HEADER_SIZE + payload.len();
    let mut bytes = [0_u8; BLOCK_SIZE];
    bytes[0..4].copy_from_slice(&NODE_MAGIC.to_le_bytes());
    bytes[4] = level;
    bytes[6..8].copy_from_slice(&count.to_le_bytes());
    bytes[8..12].copy_from_slice(&(used as u32).to_le_bytes());
    bytes[HEADER_SIZE..used].copy_from_slice(payload);
    let checksum = crc32c(&bytes[..used]);
    bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
    let block = store.allocate_blocks(1)?;
    store.write_block(block, &bytes)?;
    Ok(block)
}

fn read_u16(data: &[u8], offset: &mut usize) -> Result<u16, HolyError> {
    let bytes = data.get(*offset..*offset + 2).ok_or(HolyError::Corrupt)?;
    *offset += 2;
    Ok(u16::from_le_bytes(
        bytes.try_into().map_err(|_| HolyError::Corrupt)?,
    ))
}

fn read_u64(data: &[u8], offset: &mut usize) -> Result<u64, HolyError> {
    let bytes = data.get(*offset..*offset + 8).ok_or(HolyError::Corrupt)?;
    *offset += 8;
    Ok(u64::from_le_bytes(
        bytes.try_into().map_err(|_| HolyError::Corrupt)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use hk_block::{MemoryDisk, SECTOR_SIZE};

    #[test]
    fn round_trips_tree() {
        let mut store = Store::format(MemoryDisk::new(32768, SECTOR_SIZE)).expect("格式");
        let mut map = TreeMap::new();
        for index in 0..100_u32 {
            map.insert(index.to_le_bytes().to_vec(), vec![index as u8; 24]);
        }
        let root = store_tree(&mut store, &map).expect("写入");
        let loaded = load_tree(&store, root).expect("读取");
        assert_eq!(loaded, map);
    }
}
