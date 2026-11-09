//! Cadence 与 Meter 的固定长度快照。
//!
//! checkpoint 保存契约身份与计量水位；恢复时由 Tempo 重新准入。编码为小端
//! 定长布局，缓冲区不足时拒绝。

use hk_base::Duration;

use crate::{
    Cadence, CadenceClass, CadenceError, CadenceId, CadenceSpec, CpuMask, Lending, Meter, Weight,
};

/// Cadence 快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CadenceSnapshot {
    /// 契约标识。
    pub id: u64,
    /// 父契约标识；0 表示根契约。
    pub parent: u64,
    /// epoch。
    pub epoch: u32,
    /// 类别。
    pub class: CadenceClass,
    /// 权重。
    pub weight: u32,
    /// 预算 tick 数。
    pub budget: u64,
    /// 周期 tick 数。
    pub period: u64,
    /// 截止期 tick 数。
    pub deadline: u64,
    /// 延迟目标 tick 数。
    pub latency: u64,
    /// 亲和性位图。
    pub affinity: u64,
    /// 是否参与迁移。
    pub migratable: bool,
    /// 借贷策略。
    pub lending: Lending,
}

impl CadenceSnapshot {
    /// 编码长度。
    pub const ENCODED_LEN: usize = 80;

    /// 从契约生成快照。
    pub fn from_cadence(cadence: &Cadence) -> Self {
        Self {
            id: cadence.id().get(),
            parent: cadence.parent().map_or(0, CadenceId::get),
            epoch: cadence.epoch(),
            class: cadence.class(),
            weight: cadence.weight().get(),
            budget: cadence.budget().ticks(),
            period: cadence.period().ticks(),
            deadline: cadence.deadline().ticks(),
            latency: cadence.latency().ticks(),
            affinity: cadence.affinity().bits(),
            migratable: cadence.migratable(),
            lending: cadence.lending(),
        }
    }

    /// 还原契约。
    pub fn to_cadence(self) -> Result<Cadence, CadenceError> {
        Cadence::restore(
            CadenceId::new(self.id),
            self.parent(),
            self.epoch,
            self.spec(),
        )
    }

    /// 返回契约描述；身份字段（id/parent/epoch）由调用方决定。
    pub fn spec(self) -> CadenceSpec {
        CadenceSpec {
            class: self.class,
            weight: self.weight,
            budget: Duration::from_ticks(self.budget),
            period: Duration::from_ticks(self.period),
            deadline: Duration::from_ticks(self.deadline),
            latency: Duration::from_ticks(self.latency),
            affinity: CpuMask::from_bits(self.affinity),
            migratable: self.migratable,
            lending: self.lending,
        }
    }

    /// 返回父契约标识。
    pub fn parent(self) -> Option<CadenceId> {
        (self.parent != 0).then(|| CadenceId::new(self.parent))
    }

    /// 编码为定长小端布局。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < Self::ENCODED_LEN {
            return None;
        }
        buffer[0..8].copy_from_slice(&self.id.to_le_bytes());
        buffer[8..16].copy_from_slice(&self.parent.to_le_bytes());
        buffer[16..20].copy_from_slice(&self.epoch.to_le_bytes());
        buffer[20] = self.class.as_u8();
        buffer[21] = u8::from(self.migratable);
        buffer[22] = u8::from(self.lending.enabled());
        buffer[23] = self.lending.max_depth();
        buffer[24..28].copy_from_slice(&self.weight.to_le_bytes());
        buffer[28..32].fill(0);
        buffer[32..40].copy_from_slice(&self.budget.to_le_bytes());
        buffer[40..48].copy_from_slice(&self.period.to_le_bytes());
        buffer[48..56].copy_from_slice(&self.deadline.to_le_bytes());
        buffer[56..64].copy_from_slice(&self.latency.to_le_bytes());
        buffer[64..72].copy_from_slice(&self.affinity.to_le_bytes());
        buffer[72..80].fill(0);
        Some(Self::ENCODED_LEN)
    }

    /// 从定长小端布局解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, CadenceError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(CadenceError::SnapshotTooShort);
        }
        let class = CadenceClass::from_u8(bytes[20]).ok_or(CadenceError::UnknownClass)?;
        let lending = if bytes[22] == 0 {
            Lending::disabled()
        } else {
            Lending::with_depth(bytes[23])?
        };
        Ok(Self {
            id: u64::from_le_bytes(bytes[0..8].try_into().expect("定长")),
            parent: u64::from_le_bytes(bytes[8..16].try_into().expect("定长")),
            epoch: u32::from_le_bytes(bytes[16..20].try_into().expect("定长")),
            class,
            weight: u32::from_le_bytes(bytes[24..28].try_into().expect("定长")),
            budget: u64::from_le_bytes(bytes[32..40].try_into().expect("定长")),
            period: u64::from_le_bytes(bytes[40..48].try_into().expect("定长")),
            deadline: u64::from_le_bytes(bytes[48..56].try_into().expect("定长")),
            latency: u64::from_le_bytes(bytes[56..64].try_into().expect("定长")),
            affinity: u64::from_le_bytes(bytes[64..72].try_into().expect("定长")),
            migratable: bytes[21] != 0,
            lending,
        })
    }
}

impl Default for CadenceSnapshot {
    fn default() -> Self {
        let cadence = Cadence::new(CadenceId::new(0), CadenceClass::Latency, Weight::REFERENCE);
        Self::from_cadence(&cadence)
    }
}

/// Meter 快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterSnapshot {
    /// 实际获得 CPU 的 tick 数。
    pub consumed: u64,
    /// 等待 tick 数。
    pub waited: u64,
    /// 预算耗尽次数。
    pub throttled: u64,
    /// 超限次数。
    pub overruns: u64,
    /// 迁移次数。
    pub migrations: u64,
    /// 切换次数。
    pub switches: u64,
    /// 虚拟进度。
    pub virtual_pass: u64,
    /// 最近运行的 CPU；`u32::MAX` 表示尚未运行。
    pub last_cpu: u32,
}

impl Default for MeterSnapshot {
    fn default() -> Self {
        Meter::new().snapshot()
    }
}

impl MeterSnapshot {
    /// 编码长度。
    pub const ENCODED_LEN: usize = 64;

    /// 从计量生成快照。
    pub fn from_meter(meter: &Meter) -> Self {
        Self {
            consumed: meter.consumed(),
            waited: meter.waited(),
            throttled: meter.throttled(),
            overruns: meter.overruns(),
            migrations: meter.migrations(),
            switches: meter.switches(),
            virtual_pass: meter.virtual_pass(),
            last_cpu: meter.last_cpu(),
        }
    }

    /// 编码为定长小端布局。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < Self::ENCODED_LEN {
            return None;
        }
        buffer[0..8].copy_from_slice(&self.consumed.to_le_bytes());
        buffer[8..16].copy_from_slice(&self.waited.to_le_bytes());
        buffer[16..24].copy_from_slice(&self.throttled.to_le_bytes());
        buffer[24..32].copy_from_slice(&self.overruns.to_le_bytes());
        buffer[32..40].copy_from_slice(&self.migrations.to_le_bytes());
        buffer[40..48].copy_from_slice(&self.switches.to_le_bytes());
        buffer[48..56].copy_from_slice(&self.virtual_pass.to_le_bytes());
        buffer[56..60].copy_from_slice(&self.last_cpu.to_le_bytes());
        buffer[60..64].fill(0);
        Some(Self::ENCODED_LEN)
    }

    /// 从定长小端布局解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, CadenceError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(CadenceError::SnapshotTooShort);
        }
        Ok(Self {
            consumed: u64::from_le_bytes(bytes[0..8].try_into().expect("定长")),
            waited: u64::from_le_bytes(bytes[8..16].try_into().expect("定长")),
            throttled: u64::from_le_bytes(bytes[16..24].try_into().expect("定长")),
            overruns: u64::from_le_bytes(bytes[24..32].try_into().expect("定长")),
            migrations: u64::from_le_bytes(bytes[32..40].try_into().expect("定长")),
            switches: u64::from_le_bytes(bytes[40..48].try_into().expect("定长")),
            virtual_pass: u64::from_le_bytes(bytes[48..56].try_into().expect("定长")),
            last_cpu: u32::from_le_bytes(bytes[56..60].try_into().expect("定长")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Weight, slice_for};

    fn sample() -> Cadence {
        CadenceSpec::new(CadenceClass::Realtime, 2048)
            .with_budget(Duration::from_ticks(3), Duration::from_ticks(16))
            .with_deadline(Duration::from_ticks(8))
            .with_latency(Duration::from_ticks(4))
            .with_affinity(CpuMask::from_bits(0b0111))
            .with_migratable(false)
            .with_lending(Lending::with_depth(3).expect("深度合法"))
            .cadence(CadenceId::new(7))
            .expect("契约合法")
    }

    #[test]
    fn round_trips_cadence_snapshot() {
        let cadence = sample();
        let snapshot = CadenceSnapshot::from_cadence(&cadence);
        let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
        assert_eq!(
            snapshot.encode(&mut bytes),
            Some(CadenceSnapshot::ENCODED_LEN)
        );
        assert_eq!(CadenceSnapshot::decode(&bytes), Ok(snapshot), "快照往返");
        assert_eq!(snapshot.to_cadence(), Ok(cadence));
    }

    #[test]
    fn rejects_short_and_unknown_snapshots() {
        assert_eq!(
            CadenceSnapshot::decode(&[0_u8; 4]).err(),
            Some(CadenceError::SnapshotTooShort)
        );
        let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
        bytes[20] = 9;
        assert_eq!(
            CadenceSnapshot::decode(&bytes).err(),
            Some(CadenceError::UnknownClass)
        );
        assert_eq!(
            CadenceSnapshot::from_cadence(&sample()).encode(&mut []),
            None
        );
    }

    #[test]
    fn round_trips_meter_snapshot() {
        let mut meter = Meter::new();
        meter.charge(Duration::from_ticks(5), Weight::new(512).expect("权重合法"));
        meter.migrate(1);
        let snapshot = MeterSnapshot::from_meter(&meter);
        let mut bytes = [0_u8; MeterSnapshot::ENCODED_LEN];
        assert_eq!(
            snapshot.encode(&mut bytes),
            Some(MeterSnapshot::ENCODED_LEN)
        );
        assert_eq!(MeterSnapshot::decode(&bytes), Ok(snapshot));
        assert_eq!(Meter::restore(snapshot), meter);
        assert_eq!(slice_for(Weight::REFERENCE), crate::BASE_SLICE);
    }
}
