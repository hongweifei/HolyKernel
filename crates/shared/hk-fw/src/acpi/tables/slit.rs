//! SLIT：系统局部性距离表。
//!
//! 表头之后是一个 `u64` 局部域数量 `N`，随后是 `N × N` 个字节的距离矩阵。
//! 距离 10 表示本地，数值越大访问代价越高；固件可以省略自距离以外的项，
//! 但这里只接受完整矩阵，避免把截断表当成有效拓扑。

use super::{SIGNATURE_SLIT, table_length, u64_at};

/// SLIT 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slit {
    /// 局部域数量。
    pub localities: usize,
    /// 行优先距离矩阵。
    pub distances: alloc::vec::Vec<u8>,
}

impl Slit {
    /// 返回 `from` 到 `to` 的距离；越界时返回 `None`。
    pub fn distance(&self, from: u8, to: u8) -> Option<u8> {
        let from = usize::from(from);
        let to = usize::from(to);
        if from >= self.localities || to >= self.localities {
            return None;
        }
        self.distances.get(from * self.localities + to).copied()
    }

    /// 返回与 `node` 距离最近的节点序列（包含自身，按距离升序）。
    pub fn nearest(&self, node: u8) -> alloc::vec::Vec<(u8, u8)> {
        let mut nodes: alloc::vec::Vec<(u8, u8)> = (0..self.localities)
            .filter_map(|other| {
                self.distance(node, other as u8)
                    .map(|distance| (distance, other as u8))
            })
            .collect();
        nodes.sort_unstable();
        nodes
    }
}

/// 解析 SLIT。
pub fn parse_slit(bytes: &[u8]) -> Option<Slit> {
    let length = table_length(bytes, SIGNATURE_SLIT)?;
    let bytes = &bytes[..length];
    let localities = u64_at(bytes, super::HEADER)?;
    let localities = usize::try_from(localities).ok()?;
    if localities == 0 {
        return None;
    }
    let entries = localities.checked_mul(localities)?;
    let start = super::HEADER.checked_add(8)?;
    let end = start.checked_add(entries)?;
    if end > bytes.len() {
        return None;
    }
    // 距离 0 只允许出现在对角线以外的固件错误；保留原始值供调用方判断，
    // 解析层只校验矩阵完整。
    Some(Slit {
        localities,
        distances: bytes[start..end].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acpi::tables::tests::table;

    #[test]
    fn parses_distance_matrix() {
        let mut body = alloc::vec![0_u8; 8];
        body[..8].copy_from_slice(&2_u64.to_le_bytes());
        body.extend_from_slice(&[10, 20, 20, 10]);
        let slit = parse_slit(&table(SIGNATURE_SLIT, &body)).expect("SLIT");
        assert_eq!(slit.localities, 2);
        assert_eq!(slit.distance(0, 0), Some(10));
        assert_eq!(slit.distance(1, 0), Some(20));
        assert_eq!(slit.distance(2, 0), None);
        assert_eq!(slit.nearest(1), alloc::vec![(10, 1), (20, 0)]);
    }

    #[test]
    fn rejects_truncated_matrix() {
        let mut body = alloc::vec![0_u8; 8];
        body[..8].copy_from_slice(&4_u64.to_le_bytes());
        body.extend_from_slice(&[10, 20]);
        assert!(parse_slit(&table(SIGNATURE_SLIT, &body)).is_none());
    }
}
