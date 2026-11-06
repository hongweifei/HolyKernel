//! Seat：指针与剪贴板状态。
//!
//! 焦点仍由 Display 的窗口模型维护；Seat 持有跨客户端共享的输入与剪贴板
//! 状态，后续 Wayland/Harmony 适配器复用同一套语义。

/// 剪贴板容量。
pub(crate) const CLIPBOARD_MAX: usize = 4096;

/// 一个客户端写入的剪贴板内容。
pub(crate) struct Clipboard {
    owner: u64,
    length: usize,
    data: [u8; CLIPBOARD_MAX],
}

impl Clipboard {
    pub(crate) const fn new() -> Self {
        Self {
            owner: 0,
            length: 0,
            data: [0; CLIPBOARD_MAX],
        }
    }

    /// 写入剪贴板；超过容量时拒绝并保留原内容。
    pub(crate) fn set(&mut self, owner: u64, bytes: &[u8]) -> bool {
        if bytes.len() > CLIPBOARD_MAX {
            return false;
        }
        self.owner = owner;
        self.length = bytes.len();
        self.data[..bytes.len()].copy_from_slice(bytes);
        true
    }

    /// 当前剪贴板内容。
    pub(crate) fn get(&self) -> &[u8] {
        &self.data[..self.length]
    }

    /// 写入者。
    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }
}

/// 指针与剪贴板状态。
pub(crate) struct Seat {
    pub(crate) pointer: Option<(usize, usize)>,
    clipboard: Clipboard,
}

impl Seat {
    pub(crate) const fn new() -> Self {
        Self {
            pointer: None,
            clipboard: Clipboard::new(),
        }
    }

    pub(crate) fn set_clipboard(&mut self, owner: u64, bytes: &[u8]) -> bool {
        self.clipboard.set(owner, bytes)
    }

    pub(crate) fn clipboard(&self) -> &[u8] {
        self.clipboard.get()
    }

    pub(crate) fn clipboard_owner(&self) -> u64 {
        self.clipboard.owner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_round_trip() {
        let mut seat = Seat::new();
        assert!(seat.set_clipboard(7, b"holy"));
        assert_eq!(seat.clipboard(), b"holy");
        assert_eq!(seat.clipboard_owner(), 7);
    }

    #[test]
    fn clipboard_rejects_oversize_without_clobbering() {
        let mut seat = Seat::new();
        assert!(seat.set_clipboard(1, b"keep"));
        let oversized = [0_u8; CLIPBOARD_MAX + 1];
        assert!(!seat.set_clipboard(2, &oversized));
        assert_eq!(seat.clipboard(), b"keep");
        assert_eq!(seat.clipboard_owner(), 1);
    }

    #[test]
    fn empty_clipboard_is_valid() {
        let mut seat = Seat::new();
        assert!(seat.set_clipboard(3, b""));
        assert!(seat.clipboard().is_empty());
        assert_eq!(seat.clipboard_owner(), 3);
    }

    #[test]
    fn pointer_defaults_to_none() {
        let seat = Seat::new();
        assert_eq!(seat.pointer, None);
    }
}
