//! 借用契约选择测试。

use hk_cadence::CadenceClass;

use crate::task::TaskId;
use crate::tcb::LendingFrame;

use super::cadence::lending_contract;

fn frame(class: CadenceClass, key: u64, slice: u64, latency: u64) -> LendingFrame {
    LendingFrame {
        caller: TaskId::new(1),
        sequence: 1,
        class,
        key,
        slice,
        latency,
    }
}

#[test]
fn uses_caller_contract_when_class_is_stronger() {
    let frame = frame(CadenceClass::Realtime, 8_000, 2_000, 1_000);
    assert_eq!(
        lending_contract(CadenceClass::Latency, 4_000, 8_000, 4_000, Some(frame)),
        (CadenceClass::Realtime, 8_000, 2_000, 1_000)
    );
}

#[test]
fn uses_caller_key_within_same_class() {
    let frame = frame(CadenceClass::Latency, 1_000, 1_000, 500);
    assert_eq!(
        lending_contract(CadenceClass::Latency, 4_000, 8_000, 4_000, Some(frame)),
        (CadenceClass::Latency, 1_000, 1_000, 500)
    );
}

#[test]
fn keeps_server_when_it_is_stronger() {
    let frame = frame(CadenceClass::Throughput, 0, 8_000, 0);
    assert_eq!(
        lending_contract(CadenceClass::Realtime, 0, 2_000, 1_000, Some(frame)),
        (CadenceClass::Realtime, 0, 2_000, 1_000)
    );
}

#[test]
fn ties_take_shorter_slice_and_tighter_latency() {
    let frame = frame(CadenceClass::Latency, 5_000, 1_000, 500);
    assert_eq!(
        lending_contract(CadenceClass::Latency, 5_000, 2_000, 2_000, Some(frame)),
        (CadenceClass::Latency, 5_000, 1_000, 500)
    );
}
