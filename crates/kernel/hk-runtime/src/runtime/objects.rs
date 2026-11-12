//! HMP 对象图枚举：任务持有的句柄与描述。
//!
//! 枚举只读取句柄表与对象描述，不改动授权。`SYS_OBJECT_ENUM` 与检查点快照
//! 共用同一份记录；恢复后由 Root/Memory Server 重新授予实际句柄。

use hk_checkpoint::ObjectSnapshot;
use hk_handle::ObjectKind;

use super::*;

/// 对象记录在用户缓冲区中的编码长度。
pub(crate) const OBJECT_RECORD_BYTES: usize = 64;
/// 记录类别：Capacity。
const OBJECT_CAPACITY: u8 = 0;
/// 记录类别：Frame。
const OBJECT_FRAME: u8 = 1;
/// 记录类别：类型化能力。
const OBJECT_CAPABILITY: u8 = 2;
/// 记录类别：Region。
const OBJECT_REGION: u8 = 3;
/// 记录类别：其它转移句柄。
const OBJECT_OTHER: u8 = 4;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 枚举线程槽位持有的 HMP 对象；顺序为 Capacity、能力、Frame、转移句柄、Region。
    pub(crate) fn object_records(&self, index: usize) -> Vec<ObjectSnapshot> {
        let Some(task) = self.tasks.get(index) else {
            return Vec::new();
        };
        let owner = task.tcb;
        let mut records = Vec::new();
        for (slot, entry) in task.capacities.iter().enumerate() {
            let Some(handle) = entry else { continue };
            let Ok((range, _kind, page_size, limit, _used)) =
                self.kernel.capacity_info(owner, *handle)
            else {
                continue;
            };
            records.push(ObjectSnapshot {
                kind: OBJECT_CAPACITY,
                rights: self
                    .kernel
                    .handle_rights(owner, *handle, ObjectKind::Capacity)
                    .unwrap_or(Rights::empty())
                    .bits(),
                number: (CAPACITY_HANDLE_BASE + slot + 1) as u32,
                owner: owner.get(),
                object: self
                    .kernel
                    .handle_id(owner, *handle)
                    .map_or(0, |id| id.get()),
                size: limit,
                phys: range.start().get(),
                detail: page_size as u64,
            });
        }
        for (slot, entry) in task.capabilities.iter().enumerate() {
            let Some(entry) = entry else { continue };
            let Ok((capability, backing)) = self.kernel.capability(owner, entry.handle) else {
                continue;
            };
            records.push(ObjectSnapshot {
                kind: OBJECT_CAPABILITY,
                rights: self
                    .kernel
                    .handle_rights(owner, entry.handle, ObjectKind::Capability)
                    .unwrap_or(Rights::empty())
                    .bits(),
                number: (CAPABILITY_HANDLE_BASE + slot + 1) as u32,
                owner: owner.get(),
                object: self
                    .kernel
                    .handle_id(owner, entry.handle)
                    .map_or(0, |id| id.get()),
                size: capability.size(),
                phys: backing.map_or(0, |range| range.start().get()),
                detail: u64::from(capability.kind() as u8),
            });
        }
        for (slot, entry) in task.frames.iter().enumerate() {
            let Some(entry) = entry else { continue };
            let Ok((range, page_size, capacity, _refs, _pinned)) =
                self.kernel.frame_info(owner, entry.handle)
            else {
                continue;
            };
            records.push(ObjectSnapshot {
                kind: OBJECT_FRAME,
                rights: self
                    .kernel
                    .handle_rights(owner, entry.handle, ObjectKind::Frame)
                    .unwrap_or(Rights::empty())
                    .bits(),
                number: (FRAME_HANDLE_BASE + slot + 1) as u32,
                owner: owner.get(),
                object: self
                    .kernel
                    .handle_id(owner, entry.handle)
                    .map_or(0, |id| id.get()),
                size: range.len(),
                phys: range.start().get(),
                detail: (capacity.get() << 16) | page_size as u64,
            });
        }
        for (slot, entry) in task.extra.iter().enumerate() {
            let Some(entry) = entry else { continue };
            let kind = self.kernel.handle_kind(owner, entry.handle);
            let (size, phys, detail) = match kind {
                Some(ObjectKind::Frame) => self
                    .kernel
                    .frame_info(owner, entry.handle)
                    .map_or((0, 0, OBJECT_FRAME as u64), |(range, page, _, _, _)| {
                        (range.len(), range.start().get(), page as u64)
                    }),
                Some(ObjectKind::Capability) => self.kernel.capability(owner, entry.handle).map_or(
                    (0, 0, OBJECT_CAPABILITY as u64),
                    |(capability, backing)| {
                        (
                            capability.size(),
                            backing.map_or(0, |range| range.start().get()),
                            u64::from(capability.kind() as u8),
                        )
                    },
                ),
                _ => (0, 0, 0),
            };
            records.push(ObjectSnapshot {
                kind: OBJECT_OTHER,
                rights: self
                    .kernel
                    .handle_rights(owner, entry.handle, kind.unwrap_or(ObjectKind::Frame))
                    .unwrap_or(Rights::empty())
                    .bits(),
                number: (EXTRA_HANDLE_BASE + slot + 1) as u32,
                owner: owner.get(),
                object: self
                    .kernel
                    .handle_id(owner, entry.handle)
                    .map_or(0, |id| id.get()),
                size,
                phys,
                detail,
            });
        }
        if task.leader == index {
            for (slot, region) in self.process(index).regions.iter().enumerate() {
                let Some(region) = region else { continue };
                records.push(ObjectSnapshot {
                    kind: OBJECT_REGION,
                    rights: region.rights.bits(),
                    number: (REGION_HANDLE_BASE + slot + 1) as u32,
                    owner: region.owner.get(),
                    object: self
                        .kernel
                        .handle_id(region.owner, region.handle)
                        .map_or(0, |id| id.get()),
                    size: region.len as u64,
                    phys: region.offset,
                    detail: region.virt,
                });
            }
        }
        records
    }

    /// 编码一条对象记录。
    pub(crate) fn encode_object_record(record: &ObjectSnapshot) -> [u8; OBJECT_RECORD_BYTES] {
        let mut bytes = [0_u8; OBJECT_RECORD_BYTES];
        bytes[0] = record.kind;
        bytes[4..8].copy_from_slice(&record.rights.to_le_bytes());
        bytes[8..12].copy_from_slice(&record.number.to_le_bytes());
        bytes[16..24].copy_from_slice(&record.owner.to_le_bytes());
        bytes[24..32].copy_from_slice(&record.object.to_le_bytes());
        bytes[32..40].copy_from_slice(&record.size.to_le_bytes());
        bytes[40..48].copy_from_slice(&record.phys.to_le_bytes());
        bytes[48..56].copy_from_slice(&record.detail.to_le_bytes());
        bytes
    }

    /// `SYS_OBJECT_ENUM(index, out, capacity)`。
    pub(crate) fn object_enum(&mut self, index: usize, frame: &mut C) -> Switch {
        let at = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let records = self.object_records(index);
        if out == 0 {
            frame.set_result(records.len() as u64);
            return Switch::Continue;
        }
        if capacity < OBJECT_RECORD_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(record) = records.get(at) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let bytes = Self::encode_object_record(record);
        frame.set_result(if self.platform.write_user(out, &bytes) {
            OBJECT_RECORD_BYTES as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}
