//! 原生镜像构建。

use alloc::vec::Vec;

use crate::format::{
    CAPABILITY_LEN, DEPENDENCY_LEN, HEADER_SIZE, MAGIC, RECORD_CAPABILITY, RECORD_DEPENDENCY,
    RECORD_REGION, RECORD_SYMBOL, RECORD_TASK, REGION_LEN, SYMBOL_LEN, TASK_LEN, VERSION, crc32c,
    push_u16, push_u32, push_u64, write_u16, write_u32, write_u64,
};
use crate::types::{CapabilityData, DependencyData, RegionData, SymbolData, TaskData};

/// 构建镜像。
pub fn build(
    task: TaskData<'_>,
    regions: &[RegionData<'_>],
    capabilities: &[CapabilityData<'_>],
    symbols: &[SymbolData<'_>],
    dependencies: &[DependencyData<'_>],
) -> Vec<u8> {
    let mut records = Vec::new();
    let mut record_count = 0_usize;
    let mut task_payload = Vec::new();
    push_u32(&mut task_payload, task.role);
    push_u64(&mut task_payload, task.entry);
    push_u32(&mut task_payload, task.stack_size);
    push_u32(&mut task_payload, task.arena_size);
    push_u16(&mut task_payload, task.name.len() as u16);
    task_payload.extend_from_slice(task.name.as_bytes());
    push_record(&mut records, RECORD_TASK, &task_payload);
    record_count += 1;

    for region in regions {
        let mut payload = Vec::new();
        push_u64(&mut payload, region.virtual_address);
        push_u64(
            &mut payload,
            region.memory_size.max(region.data.len() as u64),
        );
        push_u32(&mut payload, region.flags);
        push_u32(&mut payload, region.data.len() as u32);
        payload.extend_from_slice(region.data);
        push_record(&mut records, RECORD_REGION, &payload);
        record_count += 1;
    }
    for capability in capabilities {
        let mut payload = Vec::new();
        payload.push(capability.kind);
        push_u32(&mut payload, capability.rights);
        push_u16(&mut payload, capability.name.len() as u16);
        payload.extend_from_slice(capability.name.as_bytes());
        push_record(&mut records, RECORD_CAPABILITY, &payload);
        record_count += 1;
    }
    for symbol in symbols {
        let mut payload = Vec::new();
        push_u64(&mut payload, symbol.address);
        push_u16(&mut payload, symbol.name.len() as u16);
        payload.extend_from_slice(symbol.name.as_bytes());
        push_record(&mut records, RECORD_SYMBOL, &payload);
        record_count += 1;
    }
    for dependency in dependencies {
        let mut payload = Vec::new();
        push_u16(&mut payload, dependency.min_version);
        push_u16(&mut payload, dependency.name.len() as u16);
        payload.extend_from_slice(dependency.name.as_bytes());
        push_record(&mut records, RECORD_DEPENDENCY, &payload);
        record_count += 1;
    }

    let checksum = crc32c(&records);
    let total = HEADER_SIZE + records.len();
    let mut image = alloc::vec![0; HEADER_SIZE];
    image[..4].copy_from_slice(&MAGIC);
    write_u16(&mut image, 4, VERSION);
    write_u16(&mut image, 6, HEADER_SIZE as u16);
    write_u32(&mut image, 8, record_count as u32);
    write_u32(&mut image, 12, checksum);
    write_u64(&mut image, 16, u64::from(checksum));
    write_u32(&mut image, 24, total as u32);
    image.extend_from_slice(&records);
    image
}

fn push_record(records: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    push_u16(records, kind);
    push_u16(records, 0);
    push_u32(records, payload.len() as u32);
    records.extend_from_slice(payload);
    while !records.len().is_multiple_of(4) {
        records.push(0);
    }
}

// 保持布局常量在构建模块可见，避免未使用告警的同时固定记录长度。
const _: () = {
    assert!(TASK_LEN == 22);
    assert!(REGION_LEN == 24);
    assert!(CAPABILITY_LEN == 7);
    assert!(SYMBOL_LEN == 10);
    assert!(DEPENDENCY_LEN == 4);
};
