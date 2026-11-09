//! Beat 调度核心：每 CPU 一组类别队列，按权重记账并在空闲时窃取。
//!
//! 每个 TCB 在所属类别中按 `(键, 入队序号, TCB)` 排序：实时类是绝对截止期，
//! 低延迟类是虚拟截止期，吞吐类是虚拟进度。入队时按亲和性与当前负载选择
//! CPU；本 CPU 无任务时从最忙的 CPU 窃取一个可迁移 TCB。

use alloc::collections::{BTreeMap, BTreeSet};

use hk_cadence::{CadenceClass, NO_CPU};
use hk_hal::MAX_CPUS;

use crate::tcb::TcbId;

const CLASSES: [CadenceClass; CadenceClass::COUNT] = [
    CadenceClass::Realtime,
    CadenceClass::Latency,
    CadenceClass::Throughput,
    CadenceClass::Idle,
];

/// 一次窃取得到的条目：TCB、类别、键、入队序号与权重。
type Stolen = (TcbId, CadenceClass, u64, u64, u64);

/// 单个 CPU 的类别队列。
struct Beat {
    queues: [BTreeSet<(u64, u64, u64)>; CadenceClass::COUNT],
    placements: BTreeMap<u64, (CadenceClass, u64, u64, u64)>,
    current: Option<TcbId>,
    runnable: usize,
    class_runnable: [usize; CadenceClass::COUNT],
    load: u64,
}

impl Beat {
    fn new() -> Self {
        Self {
            queues: core::array::from_fn(|_| BTreeSet::new()),
            placements: BTreeMap::new(),
            current: None,
            runnable: 0,
            class_runnable: [0; CadenceClass::COUNT],
            load: 0,
        }
    }

    fn insert(&mut self, id: TcbId, class: CadenceClass, key: u64, sequence: u64, weight: u64) {
        self.queues[class.as_u8() as usize].insert((key, sequence, id.get()));
        self.placements
            .insert(id.get(), (class, key, sequence, weight));
        self.runnable += 1;
        self.class_runnable[class.as_u8() as usize] += 1;
        self.load = self.load.saturating_add(weight);
    }

    fn remove(&mut self, id: TcbId) -> bool {
        let Some((class, key, sequence, weight)) = self.placements.remove(&id.get()) else {
            return false;
        };
        self.queues[class.as_u8() as usize].remove(&(key, sequence, id.get()));
        self.runnable -= 1;
        self.class_runnable[class.as_u8() as usize] -= 1;
        self.load = self.load.saturating_sub(weight);
        true
    }

    fn take_allowed<F>(&mut self, allowed: &mut F, classes: &[CadenceClass]) -> Option<Stolen>
    where
        F: FnMut(TcbId) -> bool,
    {
        for class in classes.iter().copied() {
            let queue = &mut self.queues[class.as_u8() as usize];
            let Some((key, sequence, id)) = queue
                .iter()
                .find(|(_, _, id)| allowed(TcbId::new(*id)))
                .copied()
            else {
                continue;
            };
            let (_, _, _, weight) = *self.placements.get(&id).expect("队列条目缺少归属记录");
            queue.remove(&(key, sequence, id));
            self.placements.remove(&id);
            self.runnable -= 1;
            self.class_runnable[class.as_u8() as usize] -= 1;
            self.load = self.load.saturating_sub(weight);
            return Some((TcbId::new(id), class, key, sequence, weight));
        }
        None
    }
}

/// 每 CPU Beat 调度器。
pub struct Scheduler {
    beats: [Beat; MAX_CPUS],
    homed: BTreeMap<u64, usize>,
    sequence: u64,
    online: usize,
}

impl Scheduler {
    /// 构造空调度器。
    pub fn new() -> Self {
        Self {
            beats: core::array::from_fn(|_| Beat::new()),
            homed: BTreeMap::new(),
            sequence: 0,
            online: 1,
        }
    }

    /// 设置在线 CPU 数；入队与窃取只考虑这些 CPU。
    pub fn set_online(&mut self, online: usize) {
        self.online = online.clamp(1, MAX_CPUS);
    }

    /// 返回在线 CPU 数。
    pub fn online(&self) -> usize {
        self.online
    }

    /// 把 TCB 加入指定 CPU 的类别队列；已在队列中时返回 `false`。
    pub fn enqueue(
        &mut self,
        id: TcbId,
        class: CadenceClass,
        key: u64,
        cpu: usize,
        weight: u64,
    ) -> bool {
        if self.homed.contains_key(&id.get()) {
            return false;
        }
        let cpu = cpu.min(MAX_CPUS - 1);
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.beats[cpu].insert(id, class, key, sequence, weight);
        self.homed.insert(id.get(), cpu);
        true
    }

    /// 按亲和性与当前负载选择 CPU 并入队。
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_best(
        &mut self,
        id: TcbId,
        class: CadenceClass,
        key: u64,
        weight: u64,
        affinity: u64,
        last_cpu: u32,
        migratable: bool,
    ) -> bool {
        let cpu = self.choose_cpu(affinity, last_cpu, migratable);
        self.enqueue(id, class, key, cpu, weight)
    }

    /// 从队列移除 TCB。
    pub fn remove(&mut self, id: TcbId) -> bool {
        let Some(cpu) = self.homed.remove(&id.get()) else {
            return false;
        };
        self.beats[cpu].remove(id)
    }

    /// 按亲和性与负载选择入队 CPU。
    pub fn choose_cpu(&self, affinity: u64, last_cpu: u32, migratable: bool) -> usize {
        if !migratable && last_cpu != NO_CPU && affinity & (1_u64 << last_cpu) != 0 {
            return last_cpu as usize;
        }
        let mut best = 0_usize;
        let mut best_load = u64::MAX;
        for cpu in 0..self.online {
            if affinity & (1_u64 << cpu) == 0 {
                continue;
            }
            let load = self.beats[cpu].load;
            if load < best_load {
                best = cpu;
                best_load = load;
            }
        }
        best
    }

    /// 取下一个满足 `allowed` 的 TCB；本 CPU 为空时先从最忙的 CPU 窃取。
    pub fn pick_with<F>(&mut self, cpu: usize, mut allowed: F) -> Option<(TcbId, CadenceClass)>
    where
        F: FnMut(TcbId) -> bool,
    {
        assert!(cpu < MAX_CPUS, "CPU 索引越界");
        let real = &CLASSES[..CadenceClass::Idle as usize];
        if let Some((tcb, class, _, _, _)) = self.beats[cpu].take_allowed(&mut allowed, real) {
            self.homed.remove(&tcb.get());
            self.beats[cpu].current = Some(tcb);
            return Some((tcb, class));
        }
        // 本地没有真实任务时先窃取，避免 idle TCB 让本 CPU 抢先进入空闲。
        if self.steal_into(cpu, &mut allowed)
            && let Some((tcb, class, _, _, _)) = self.beats[cpu].take_allowed(&mut allowed, real)
        {
            self.homed.remove(&tcb.get());
            self.beats[cpu].current = Some(tcb);
            return Some((tcb, class));
        }
        // 最后才选择 idle TCB。
        let (tcb, class, _, _, _) = self.beats[cpu].take_allowed(&mut allowed, &CLASSES)?;
        self.homed.remove(&tcb.get());
        self.beats[cpu].current = Some(tcb);
        Some((tcb, class))
    }

    fn steal_into<F>(&mut self, cpu: usize, allowed: &mut F) -> bool
    where
        F: FnMut(TcbId) -> bool,
    {
        let mut victim = None;
        let mut victim_load = 0;
        for other in 0..self.online {
            if other == cpu {
                continue;
            }
            let beat = &self.beats[other];
            if beat.runnable == 0 || beat.load <= victim_load {
                continue;
            }
            let migratable = beat
                .queues
                .iter()
                .any(|queue| queue.iter().any(|(_, _, id)| allowed(TcbId::new(*id))));

            if migratable {
                victim = Some(other);
                victim_load = beat.load;
            }
        }
        let Some(victim) = victim else {
            return false;
        };
        let Some((tcb, class, key, sequence, weight)) =
            self.beats[victim].take_allowed(allowed, &CLASSES)
        else {
            return false;
        };
        self.beats[cpu].insert(tcb, class, key, sequence, weight);
        self.homed.insert(tcb.get(), cpu);
        true
    }

    /// 返回 `cpu` 的当前线程。
    pub fn current(&self, cpu: usize) -> Option<TcbId> {
        self.beats[cpu].current
    }

    /// 取走 `cpu` 的当前线程记录。
    pub fn take_current(&mut self, cpu: usize) -> Option<TcbId> {
        assert!(cpu < MAX_CPUS, "CPU 索引越界");
        self.beats[cpu].current.take()
    }

    /// 线程是任意 CPU 的当前线程时清除记录。
    pub fn clear_current_if(&mut self, id: TcbId) {
        for beat in &mut self.beats {
            if beat.current == Some(id) {
                beat.current = None;
            }
        }
    }

    /// 返回就绪 TCB 总数；Idle 类不计入，避免空闲 TCB 让宿主误以为有工作。
    pub fn runnable(&self) -> usize {
        self.beats
            .iter()
            .map(|beat| {
                beat.class_runnable[..CadenceClass::Idle.as_u8() as usize]
                    .iter()
                    .sum::<usize>()
            })
            .sum()
    }

    /// 返回某个 CPU 的 idle TCB 是否在队列中。
    pub fn idle_runnable(&self, cpu: usize) -> bool {
        self.beats[cpu].class_runnable[CadenceClass::Idle.as_u8() as usize] > 0
    }

    /// 返回某个 CPU 的加权负载。
    pub fn load(&self, cpu: usize) -> u64 {
        self.beats[cpu].load
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> TcbId {
        TcbId::new(value)
    }

    #[test]
    fn picks_highest_class_first_on_same_cpu() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), CadenceClass::Throughput, 0, 0, 1);
        scheduler.enqueue(id(2), CadenceClass::Latency, 10, 0, 1);
        scheduler.enqueue(id(3), CadenceClass::Realtime, 5, 0, 1);

        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(3), CadenceClass::Realtime))
        );
        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(2), CadenceClass::Latency))
        );
        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(1), CadenceClass::Throughput))
        );
        assert_eq!(scheduler.pick_with(0, |_| true), None);
    }

    #[test]
    fn orders_by_key_within_class() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), CadenceClass::Latency, 30, 0, 1);
        scheduler.enqueue(id(2), CadenceClass::Latency, 10, 0, 1);
        scheduler.enqueue(id(3), CadenceClass::Latency, 20, 0, 1);

        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(2), CadenceClass::Latency))
        );
        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(3), CadenceClass::Latency))
        );
        assert_eq!(
            scheduler.pick_with(0, |_| true),
            Some((id(1), CadenceClass::Latency))
        );
    }

    #[test]
    fn rejects_duplicate_enqueue() {
        let mut scheduler = Scheduler::new();
        assert!(scheduler.enqueue(id(1), CadenceClass::Latency, 10, 0, 1));
        assert!(!scheduler.enqueue(id(1), CadenceClass::Latency, 20, 0, 1));
        assert_eq!(scheduler.runnable(), 1);
    }

    #[test]
    fn balances_new_tasks_across_cpus() {
        let mut scheduler = Scheduler::new();
        scheduler.set_online(2);
        let first = scheduler.choose_cpu(u64::MAX, NO_CPU, true);
        scheduler.enqueue(id(1), CadenceClass::Latency, 10, first, 1024);
        let second = scheduler.choose_cpu(u64::MAX, NO_CPU, true);
        assert_ne!(first, second);
    }

    #[test]
    fn pinned_task_keeps_last_cpu() {
        let mut scheduler = Scheduler::new();
        scheduler.set_online(3);
        assert_eq!(scheduler.choose_cpu(u64::MAX, 2, false), 2);
        assert_eq!(scheduler.choose_cpu(0b100, NO_CPU, false), 2);
    }

    #[test]
    fn steals_from_busiest_cpu_when_idle() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), CadenceClass::Latency, 10, 0, 1024);
        scheduler.enqueue(id(2), CadenceClass::Latency, 20, 0, 1024);

        // CPU1 为空，从 CPU0 窃取一个；顺序仍按键。
        assert_eq!(
            scheduler.pick_with(1, |_| true),
            Some((id(1), CadenceClass::Latency))
        );
        assert_eq!(scheduler.load(0), 1024);
        assert_eq!(scheduler.load(1), 0);
    }

    #[test]
    fn respects_allowed_predicate_when_stealing() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), CadenceClass::Latency, 10, 0, 1024);

        assert_eq!(scheduler.pick_with(1, |tcb| tcb != id(1)), None);
        assert_eq!(scheduler.runnable(), 1);
    }
}
