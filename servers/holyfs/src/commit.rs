//! 同步请求的 group commit：同一盘符只刷写一次。

use alloc::vec::Vec;

use hk_proto::vfs::STATUS_NOT_FOUND;

/// 去重盘符刷写，并按原顺序为每个等待者返回状态。
pub(crate) fn commit<F>(pending: &[(u8, u64)], mut sync: F) -> Vec<(u64, u8)>
where
    F: FnMut(u8) -> u8,
{
    let mut committed: Vec<(u8, u8)> = Vec::new();
    for &(letter, _) in pending {
        if committed.iter().any(|(done, _)| *done == letter) {
            continue;
        }
        committed.push((letter, sync(letter)));
    }
    pending
        .iter()
        .map(|&(letter, sender)| {
            let status = committed
                .iter()
                .find(|(done, _)| *done == letter)
                .map_or(STATUS_NOT_FOUND, |(_, status)| *status);
            (sender, status)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_each_drive_once() {
        let pending = [(b'H', 10), (b'I', 11), (b'H', 12), (b'I', 13)];
        let mut synced = Vec::new();
        let replies = commit(&pending, |letter| {
            synced.push(letter);
            0
        });
        assert_eq!(synced, [b'H', b'I']);
        assert_eq!(replies, [(10, 0), (11, 0), (12, 0), (13, 0)]);
    }

    #[test]
    fn preserves_per_drive_status() {
        let pending = [(b'H', 1), (b'I', 2), (b'H', 3)];
        let replies = commit(&pending, |letter| if letter == b'H' { 9 } else { 4 });
        assert_eq!(replies, [(1, 9), (2, 4), (3, 9)]);
    }
}
