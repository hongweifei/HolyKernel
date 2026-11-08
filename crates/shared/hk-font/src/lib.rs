//! 字体：TrueType 解析、轮廓栅格化、等宽位图与 8×16 回退。
//!
//! 默认构建包含可缩放字体：内置 Adwaita Sans（SIL OFL 1.1），缺少字形时回退到
//! `efont_cn_16` 位图字体并按目标字号缩放，栅格化使用固定缓冲与 4×4 超采样。
//! 关闭 `scalable` 特性后只保留位图字体，用于不需要 TrueType 的早期启动链路。
//!
//! [`default_font`]、[`default_font_scaled`] 与 [`default_monospace`] 提供首次使用时初始化
//! 并常驻的 16/32 像素可缩放字体与 16 像素等宽字体，供不指定字体的通用排版与字符网格复用。
//!
//! 内部按职责拆分：`data` 持有内置字体数据，`glyph` 定义字形状态与公开视图，
//! `raster` 负责轮廓与超采样，`cache` 维护固定槽位缓存，`font` 暴露字体实例。

#![no_std]

#[cfg(test)]
extern crate std;

mod cache;
mod data;
mod font;
mod glyph;
#[cfg(feature = "scalable")]
mod raster;

pub use font::{Font, default_font, default_font_scaled, default_monospace};
pub use glyph::{Glyph, Metric};
