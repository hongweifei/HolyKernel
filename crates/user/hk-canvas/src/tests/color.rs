//! 颜色混合测试。

use crate::mix;

#[test]
fn mix_interpolates_each_channel() {
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 0), 0x00_00_00);
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 255), 0xFF_FF_FF);
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 128), 0x80_80_80);
    assert_eq!(mix(0x10_20_30, 0x10_20_30, 200), 0x10_20_30);
}

#[test]
fn mix_rounds_each_channel_down() {
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 1), 0x01_01_01);
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 254), 0xFE_FE_FE);
}
