//! 帧银行内部空闲表：按节点切分、查找、切出与合并。

use ::alloc::vec::Vec;

use hk_hal::{PhysAddr, PhysRange};

use super::{Inner, MAX_NODES, OwnedWindow, align_up_u64};
use crate::hmp::CapacityId;

impl Inner {
    /// 返回物理地址所属节点；未配置拓扑时归入节点 0。
    pub(super) fn node_for(&self, addr: u64) -> u8 {
        for (range, node) in &self.nodes {
            if addr >= range.start().get() && addr < range.end().get() {
                return *node;
            }
        }
        0
    }

    /// 按节点边界切分现有空闲条目，并清空节点统计。
    pub(super) fn split_free_by_nodes(&mut self) {
        self.by_node = [0; MAX_NODES];
        if self.nodes.is_empty() {
            return;
        }
        let ranges: Vec<PhysRange> = self.free_by_start.values().copied().collect();
        self.free_by_start.clear();
        self.free_by_len.clear();
        for range in ranges {
            let start = range.start().get();
            let end = range.end().get();
            let mut cuts = ::alloc::vec![start, end];
            for (node_range, _) in &self.nodes {
                let node_start = node_range.start().get();
                let node_end = node_range.end().get();
                if node_start > start && node_start < end {
                    cuts.push(node_start);
                }
                if node_end > start && node_end < end {
                    cuts.push(node_end);
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            for window in cuts.windows(2) {
                let piece_start = window[0];
                let piece_end = window[1];
                if piece_end > piece_start {
                    self.insert_free(PhysRange::new(
                        PhysAddr::new(piece_start),
                        piece_end - piece_start,
                    ));
                }
            }
        }
    }

    /// 把一段区间按节点边界切成多段，保证空闲条目不跨节点。
    pub(super) fn node_pieces(&self, range: PhysRange) -> Vec<PhysRange> {
        if self.nodes.is_empty() {
            return ::alloc::vec![range];
        }
        let start = range.start().get();
        let end = range.end().get();
        let mut cuts = ::alloc::vec![start, end];
        for (node_range, _) in &self.nodes {
            let node_start = node_range.start().get();
            let node_end = node_range.end().get();
            if node_start > start && node_start < end {
                cuts.push(node_start);
            }
            if node_end > start && node_end < end {
                cuts.push(node_end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2)
            .filter(|window| window[1] > window[0])
            .map(|window| PhysRange::new(PhysAddr::new(window[0]), window[1] - window[0]))
            .collect()
    }

    pub(super) fn insert_free(&mut self, range: PhysRange) {
        let start = range.start().get();
        if let Some(previous) = self.free_by_start.get(&start)
            && previous.len() != range.len()
            && let Some(starts) = self.free_by_len.get_mut(&previous.len())
        {
            starts.remove(&start);
        }
        self.free_by_len
            .entry(range.len())
            .or_default()
            .insert(start);
        self.free_by_start.insert(start, range);
    }

    pub(super) fn take_free(&mut self, start: u64) -> Option<PhysRange> {
        let range = self.free_by_start.remove(&start)?;
        let length = range.len();
        if let Some(starts) = self.free_by_len.get_mut(&length) {
            starts.remove(&start);
            if starts.is_empty() {
                self.free_by_len.remove(&length);
            }
        }
        Some(range)
    }

    /// 在指定容量登记的窗口内寻找一段足够长的空闲区间。
    ///
    /// 空闲条目可能与相邻容量合并成大块，也可能被窗口边界切成多段，因此不能
    /// 只看单个条目：这里把落在窗口内的空闲段收集起来、按起点排序并合并相邻
    /// 段，再找足够长的连续区间。相邻且属于同一容量的窗口先合并为一段，避免
    /// 同一容量自己的连续空闲空间被窗口边界切成不可用的碎块。
    pub(super) fn find_free_in_owned(
        &self,
        owner: CapacityId,
        length: u64,
        align: u64,
        node: Option<u8>,
    ) -> Option<u64> {
        let mut windows: Vec<(u64, u64)> = self
            .owned
            .values()
            .filter(|window| window.capacity == owner)
            .map(|window| (window.range.start().get(), window.range.end().get()))
            .collect();
        windows.sort_unstable();
        let mut spans: Vec<(u64, u64)> = Vec::with_capacity(windows.len());
        for (start, end) in windows {
            match spans.last_mut() {
                Some(last) if last.1 == start => last.1 = end,
                _ => spans.push((start, end)),
            }
        }
        for (ws, we) in spans {
            let mut pieces: Vec<(u64, u64)> = self
                .free_by_start
                .values()
                .filter_map(|range| {
                    let start = range.start().get().max(ws);
                    let end = range.end().get().min(we);
                    (end > start).then_some((start, end))
                })
                .collect();
            pieces.sort_unstable();
            let mut run: Option<(u64, u64)> = None;
            for (start, end) in pieces {
                // 与当前运行段相邻（或重叠）时延长它，否则从本段开始新运行段。
                // 每一段都要检查长度，末尾的长空闲段不能被跳过。
                let (run_start, run_end) = match run {
                    Some((run_start, run_end))
                        if run_end >= start && self.node_for(run_start) == self.node_for(start) =>
                    {
                        (run_start, run_end.max(end))
                    }
                    _ => (start, end),
                };
                if node.is_some_and(|node| self.node_for(run_start) != node) {
                    continue;
                }
                if run_end.saturating_sub(run_start) >= length
                    && let Some(aligned) = align_up_u64(run_start, align)
                    && aligned.saturating_add(length) <= run_end
                {
                    return Some(aligned);
                }
                run = Some((run_start, run_end));
            }
        }
        None
    }

    /// 从空闲表中切出 `[start, start+len)`，必要时拆分条目。
    ///
    /// 区间可能横跨多个相邻空闲条目；先把这些条目整体取出，再按前后残段
    /// 分别插回。
    pub(super) fn carve_free(&mut self, start: u64, len: u64) -> Option<PhysRange> {
        let end = start.checked_add(len)?;
        // 收集所有与区间相交的空闲条目。
        let covered: Vec<(u64, PhysRange)> = self
            .free_by_start
            .iter()
            .filter(|(_, range)| range.start().get() < end && start < range.end().get())
            .map(|(key, range)| (*key, *range))
            .collect();
        let (first_start, first_end) = {
            let mut begin = u64::MAX;
            let mut finish = 0;
            for (_, range) in &covered {
                begin = begin.min(range.start().get());
                finish = finish.max(range.end().get());
            }
            (begin, finish)
        };
        if first_start > start || first_end < end {
            return None;
        }
        // 覆盖范围必须连续无洞，否则切出的区间里混着别人的页。
        let mut sorted: Vec<(u64, u64)> = covered
            .iter()
            .map(|(_, range)| (range.start().get(), range.end().get()))
            .collect();
        sorted.sort_unstable();
        let mut reach = first_start;
        for (piece_start, piece_end) in &sorted {
            if *piece_start > reach {
                return None;
            }
            reach = reach.max(*piece_end);
        }
        if reach < end {
            return None;
        }
        for (key, _) in &covered {
            self.take_free(*key)?;
        }
        if start > first_start {
            self.insert_free(PhysRange::new(
                PhysAddr::new(first_start),
                start - first_start,
            ));
        }
        let tail = first_end.saturating_sub(end);
        if tail > 0 {
            self.insert_free(PhysRange::new(PhysAddr::new(end), tail));
        }
        Some(PhysRange::new(PhysAddr::new(start), len))
    }

    /// 在未登记给任何容量的空闲区间内寻找一段足够长的区间。
    ///
    /// 先把相邻空闲段合并成连续运行段，再跳过与已登记窗口相交的部分，避免
    /// 整块大区间因含有一小段归属区而被整体弃用。
    pub(super) fn find_free_unowned(
        &self,
        length: u64,
        align: u64,
        node: Option<u8>,
    ) -> Option<u64> {
        let mut spans: Vec<(u64, u64)> = self
            .free_by_start
            .values()
            .map(|range| (range.start().get(), range.end().get()))
            .collect();
        spans.sort_unstable();
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for (start, end) in spans {
            match runs.last_mut() {
                Some(last) if last.1 == start && self.node_for(last.0) == self.node_for(start) => {
                    last.1 = end;
                }
                _ => runs.push((start, end)),
            }
        }
        for (start, end) in runs {
            if node.is_some_and(|node| self.node_for(start) != node) {
                continue;
            }
            let mut cursor = align_up_u64(start, align)?;
            while cursor.saturating_add(length) <= end {
                let blocked = self
                    .owned
                    .values()
                    .filter_map(|window| {
                        let ws = window.range.start().get();
                        let we = window.range.end().get();
                        (ws < cursor + length && cursor < we).then_some(we)
                    })
                    .max();
                match blocked {
                    Some(next) if next <= end => cursor = align_up_u64(next, align)?,
                    Some(_) => break,
                    None => return Some(cursor),
                }
            }
        }
        None
    }

    /// 返回指定容量名下的已用字节数；优先读该容量的 Ledger。
    pub(super) fn used_for(&self, owner: CapacityId) -> u64 {
        if let Some(ledger) = self.ledgers.get(&owner) {
            return ledger.charged();
        }
        self.allocations
            .values()
            .filter(|allocation| allocation.owner == Some(owner))
            .fold(0, |sum, allocation| {
                sum.saturating_add(allocation.range.len())
            })
    }

    /// 把一段窗口归到指定容量；优先扩张相邻窗口，否则登记为新窗口。
    pub(super) fn attach_window(&mut self, owner: CapacityId, range: PhysRange) {
        let adjacent = self
            .owned
            .iter()
            .find(|(_, window)| {
                window.capacity == owner
                    && (window.range.end().get() == range.start().get()
                        || range.end().get() == window.range.start().get())
            })
            .map(|(&start, _)| start);
        if let Some(start) = adjacent {
            if let Some(window) = self.owned.get_mut(&start) {
                let begin = window.range.start().get().min(range.start().get());
                let end = window.range.end().get().max(range.end().get());
                window.range = PhysRange::new(PhysAddr::new(begin), end - begin);
            }
            return;
        }
        self.owned.insert(
            range.start().get(),
            OwnedWindow {
                capacity: owner,
                range,
            },
        );
    }

    /// 把一段区间放回空闲表并与相邻区间合并。
    pub(super) fn coalesce_free(&mut self, range: PhysRange) {
        let start = range.start().get();
        let mut merged = range;
        if let Some(previous) = self
            .free_by_start
            .range(..start)
            .next_back()
            .map(|(_, range)| *range)
            && previous.end().get() == merged.start().get()
            && self.node_for(previous.start().get()) == self.node_for(merged.start().get())
        {
            self.take_free(previous.start().get());
            merged = PhysRange::new(
                previous.start(),
                previous.len().saturating_add(merged.len()),
            );
        }
        let next_start = merged.end().get();
        if let Some(next) = self
            .free_by_start
            .range(next_start..)
            .next()
            .map(|(_, range)| *range)
            && next.start().get() == next_start
            && self.node_for(next.start().get()) == self.node_for(merged.start().get())
        {
            self.take_free(next.start().get());
            merged = PhysRange::new(merged.start(), merged.len().saturating_add(next.len()));
        }
        self.insert_free(merged);
    }
}
