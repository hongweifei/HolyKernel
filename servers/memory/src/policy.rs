//! Memory Server 的回收授权与重启恢复策略。
//!
//! 这些判定与 IPC 解耦，宿主测试和裸机服务共用同一份规则。

/// 回收请求必须携带与能力对象当前绑定令牌一致的非零令牌。
pub fn reclaim_authorized(tracked: Option<u64>, requested: u64) -> bool {
    tracked.is_some_and(|token| token != 0 && token == requested)
}

/// 内核标记为已换出的页缺少 Provider 存储内容时，请求必须失败。
///
/// 未换出的首次缺页仍允许零页填充；这里只拦截“曾经保存、当前丢失”的页。
pub fn swapped_page_missing(flags: u64, stored: bool) -> bool {
    flags & u64::from(hk_syscall::PROVIDER_FAULT_SWAPPED) != 0 && !stored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reclaim_requires_matching_nonzero_token() {
        assert!(reclaim_authorized(Some(7), 7));
        assert!(!reclaim_authorized(Some(7), 8));
        assert!(!reclaim_authorized(Some(0), 0));
        assert!(!reclaim_authorized(None, 7));
    }

    #[test]
    fn missing_swapped_page_is_rejected() {
        let swapped = u64::from(hk_syscall::PROVIDER_FAULT_SWAPPED);
        assert!(swapped_page_missing(swapped, false));
        assert!(!swapped_page_missing(swapped, true));
        assert!(!swapped_page_missing(0, false));
    }
}
