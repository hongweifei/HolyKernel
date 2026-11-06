//! 按比例混合两个颜色。

/// 按 `ratio/255` 把 `to` 混入 `from`。
#[inline]
pub const fn mix(from: u32, to: u32, ratio: u8) -> u32 {
    let ratio = ratio as u32;
    let inverse = 255 - ratio;
    let mask = 0xFF;
    let red = div255(((from >> 16) & mask) * inverse + ((to >> 16) & mask) * ratio);
    let green = div255(((from >> 8) & mask) * inverse + ((to >> 8) & mask) * ratio);
    let blue = div255((from & mask) * inverse + (to & mask) * ratio);
    (red << 16) | (green << 8) | blue
}

/// 无除法的除以 255；输入上限为 `255 * 255 * 2`。
#[inline]
const fn div255(value: u32) -> u32 {
    ((value + 1) * 257) >> 16
}
