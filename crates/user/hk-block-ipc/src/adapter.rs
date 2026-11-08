//! 直写帧登记：记录客户端已映射的缓冲区，块客户端据此直接提交 I/O。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_proto::block::BUFFER_SLOTS;

const MAPPING_SLOTS: usize = BUFFER_SLOTS as usize;

static MAPPING_VIRT: [AtomicU64; MAPPING_SLOTS] = [const { AtomicU64::new(0) }; MAPPING_SLOTS];
static MAPPING_HANDLE: [AtomicU64; MAPPING_SLOTS] = [const { AtomicU64::new(0) }; MAPPING_SLOTS];
static MAPPING_LEN: [AtomicU64; MAPPING_SLOTS] = [const { AtomicU64::new(0) }; MAPPING_SLOTS];

/// 登记一个客户端缓冲区映射，供块客户端识别直写缓冲区。
pub fn register_client_mapping(virt: u64, handle: u64, len: usize) {
    for (slot, virt_slot) in MAPPING_VIRT.iter().enumerate() {
        let current = virt_slot.load(Ordering::Acquire);
        if current == 0 || current == virt {
            MAPPING_HANDLE[slot].store(handle, Ordering::Release);
            MAPPING_LEN[slot].store(len as u64, Ordering::Release);
            MAPPING_VIRT[slot].store(virt, Ordering::Release);
            return;
        }
    }
}

/// 注销一个客户端缓冲区映射。
pub fn unregister_client_mapping(virt: u64) {
    for virt_slot in &MAPPING_VIRT {
        if virt_slot.load(Ordering::Acquire) == virt {
            virt_slot.store(0, Ordering::Release);
            return;
        }
    }
}

pub(crate) fn client_mapping_at(address: u64, len: usize) -> Option<(u64, u64, usize)> {
    for (slot, virt_slot) in MAPPING_VIRT.iter().enumerate() {
        let virt = virt_slot.load(Ordering::Acquire);
        let len_limit = MAPPING_LEN[slot].load(Ordering::Acquire) as usize;
        if virt != 0 && address == virt && len <= len_limit {
            let handle = MAPPING_HANDLE[slot].load(Ordering::Acquire);
            return Some((handle, virt, len_limit));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_looks_up_and_unregisters() {
        register_client_mapping(0x1000, 7, 512);
        assert_eq!(client_mapping_at(0x1000, 512), Some((7, 0x1000, 512)));
        assert_eq!(client_mapping_at(0x1000, 513), None);
        assert_eq!(client_mapping_at(0x1004, 1), None);
        unregister_client_mapping(0x1000);
        assert_eq!(client_mapping_at(0x1000, 1), None);

        for slot in 0..MAPPING_SLOTS {
            register_client_mapping(0x2000 + slot as u64 * 0x100, slot as u64 + 1, 64);
        }
        register_client_mapping(0x9999, 99, 64);
        assert_eq!(client_mapping_at(0x9999, 64), None);
        for slot in 0..MAPPING_SLOTS {
            unregister_client_mapping(0x2000 + slot as u64 * 0x100);
        }
    }
}
