//! 字形缓存：按字符、字号与字体类型散列到固定槽位。

use spin::Mutex;

use crate::glyph::{MAX_GLYPH, State};

/// 字形缓存条目数。
const CACHE_SLOTS: usize = 128;

#[derive(Clone, Copy)]
pub(crate) struct Cached {
    pub(crate) key: u64,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) advance: i32,
    pub(crate) buffer: [u8; MAX_GLYPH * MAX_GLYPH],
}

impl Cached {
    const EMPTY: Self = Self {
        key: 0,
        width: 0,
        height: 0,
        left: 0,
        top: 0,
        advance: 0,
        buffer: [0; MAX_GLYPH * MAX_GLYPH],
    };
}

struct Cache {
    entries: [Cached; CACHE_SLOTS],
}

impl Cache {
    const fn new() -> Self {
        Self {
            entries: [Cached::EMPTY; CACHE_SLOTS],
        }
    }
}

static CACHE: Mutex<Cache> = Mutex::new(Cache::new());

/// 返回缓存 key 的起始槽位；槽位数是 2 的幂。
fn slot(key: u64) -> usize {
    let hash = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((hash >> 33) as usize) & (CACHE_SLOTS - 1)
}

pub(crate) fn get(key: u64) -> Option<Cached> {
    let cache = CACHE.lock();
    let mut index = slot(key);
    for _ in 0..CACHE_SLOTS {
        let entry = cache.entries[index];
        if entry.key == key {
            return Some(entry);
        }
        if entry.key == 0 {
            return None;
        }
        index = (index + 1) & (CACHE_SLOTS - 1);
    }
    None
}

pub(crate) fn put(key: u64, state: &State) {
    let mut cache = CACHE.lock();
    let mut index = slot(key);
    for _ in 0..CACHE_SLOTS {
        if cache.entries[index].key == key || cache.entries[index].key == 0 {
            break;
        }
        index = (index + 1) & (CACHE_SLOTS - 1);
    }
    cache.entries[index] = Cached {
        key,
        width: state.width,
        height: state.height,
        left: state.left,
        top: state.top,
        advance: state.advance,
        buffer: state.buffer,
    };
}
