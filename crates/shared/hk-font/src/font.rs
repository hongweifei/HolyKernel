//! 字体实例、度量与默认字体。

use lovyangfx_fonts::Font as BitmapFont;
use spin::{Mutex, Once};
#[cfg(feature = "scalable")]
use ttf_parser::Face;

use crate::cache;
#[cfg(feature = "scalable")]
use crate::data::FONT_DATA;
use crate::data::fallback_font;
use crate::glyph::{Glyph, MAX_GLYPH, State};
#[cfg(feature = "scalable")]
use crate::raster::rasterize;

/// 可缩放字体的字形类型；关闭 `scalable` 时没有可缩放字形。
#[cfg(feature = "scalable")]
type FaceSlot = Face<'static>;
/// 可缩放字体的字形类型；关闭 `scalable` 时没有可缩放字形。
#[cfg(not(feature = "scalable"))]
type FaceSlot = ();

/// 字体。
pub struct Font {
    #[cfg_attr(not(feature = "scalable"), allow(dead_code))]
    face: Option<FaceSlot>,
    fallback: BitmapFont<'static>,
    size: u32,
    #[cfg_attr(not(feature = "scalable"), allow(dead_code))]
    scale: f32,
    ascent: i32,
    line_height: i32,
    state: Mutex<State>,
}

/// 内置默认字体，首次使用时初始化并常驻。
static DEFAULT: Once<Font> = Once::new();
/// 内置默认字体在 200% 界面缩放下的实例。
static DEFAULT_LARGE: Once<Font> = Once::new();
/// 内置默认等宽字体。
static DEFAULT_MONOSPACE: Once<Font> = Once::new();

/// 返回内置默认字体：16 像素可缩放字体，缺字形时回退到内置位图字体。
///
/// 所有调用共享同一份字形缓存，适合不指定字体的通用文本绘制。
pub fn default_font() -> &'static Font {
    DEFAULT.call_once(|| Font::new(16))
}

/// 返回内置默认字体在界面缩放下的实例；`scale` 取 1 或 2。
///
/// 100% 缩放复用 [`default_font`]，200% 缩放使用 32 像素实例。
pub fn default_font_scaled(scale: usize) -> &'static Font {
    if scale <= 1 {
        default_font()
    } else {
        DEFAULT_LARGE.call_once(|| Font::new(32))
    }
}

/// 返回内置默认等宽字体：16 像素位图字形，ASCII 半宽、汉字全宽。
///
/// 适合字符网格与代码块排版，所有调用共享同一份字形缓存。
pub fn default_monospace() -> &'static Font {
    DEFAULT_MONOSPACE.call_once(|| Font::monospace(16))
}

impl Font {
    /// 以像素字号构造可缩放字体，缺字形时回退到内置位图字体。
    ///
    /// 关闭 `scalable` 特性时等同于 [`Font::monospace`]。
    pub fn new(size: u32) -> Self {
        let size = size.max(4);
        #[cfg(feature = "scalable")]
        {
            let face = Face::parse(FONT_DATA, 0).ok();
            let (scale, ascent, line_height) = match face.as_ref() {
                Some(face) if face.units_per_em() > 0 => {
                    let scale = size as f32 / f32::from(face.units_per_em());
                    let ascent = libm::ceilf(f32::from(face.ascender()) * scale) as i32;
                    let height = libm::ceilf(
                        f32::from(face.ascender() - face.descender() + face.line_gap()) * scale,
                    ) as i32;
                    (scale, ascent, height.max(ascent))
                }
                _ => (0.0, size as i32, size as i32 + size as i32 / 2),
            };
            Self {
                face,
                fallback: fallback_font(),
                size,
                scale,
                ascent,
                line_height,
                state: Mutex::new(State::new()),
            }
        }
        #[cfg(not(feature = "scalable"))]
        {
            Self::monospace(size)
        }
    }

    /// 以等宽位图字体构造，字符按 8 像素与 16 像素的网格推进。
    ///
    /// 字形全部来自内置位图字体，ASCII 半宽、汉字全宽，适合字符网格与代码块排版。
    pub fn monospace(size: u32) -> Self {
        let size = size.max(4);
        Self {
            face: None,
            fallback: fallback_font(),
            size,
            scale: 0.0,
            ascent: size as i32,
            line_height: size as i32 + size as i32 / 2,
            state: Mutex::new(State::new()),
        }
    }

    /// 是否成功加载可缩放字体。
    pub fn has_scalable(&self) -> bool {
        #[cfg(feature = "scalable")]
        {
            self.face.is_some() && self.scale > 0.0
        }
        #[cfg(not(feature = "scalable"))]
        {
            false
        }
    }

    /// 返回字号。
    pub fn size(&self) -> u32 {
        self.size
    }

    /// 返回基线高度。
    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    /// 返回行高。
    pub fn line_height(&self) -> i32 {
        self.line_height
    }

    /// 返回最大字形宽度（像素），用于字符网格的列宽。
    pub fn max_width(&self) -> i32 {
        self.size as i32
    }

    /// 返回最大字形高度（像素），用于字符网格的行高。
    pub fn max_height(&self) -> i32 {
        self.size as i32
    }

    /// 是否能提供该字符的字形。
    pub fn has_glyph(&self, character: char) -> bool {
        self.glyph(character).is_some()
    }

    /// 计算字符前进宽度，不栅格化字形。
    pub fn advance(&self, character: char) -> i32 {
        #[cfg(feature = "scalable")]
        if let Some(face) = self.face.as_ref()
            && let Some(id) = face.glyph_index(character)
            && let Some(advance) = face.glyph_hor_advance(id)
        {
            return (libm::roundf(f32::from(advance) * self.scale) as i32).max(1);
        }
        if let Some(source) = self.fallback.glyph(character) {
            let advance = source.metric().xadvance.max(0) as usize;
            return (advance * self.size as usize / 16).max(1) as i32;
        }
        (self.size as i32 / 2).max(1)
    }

    /// 取字形；无可用字形时返回 `None`。
    pub fn glyph(&self, character: char) -> Option<Glyph<'_>> {
        if !self.fill(character) {
            return None;
        }
        Some(Glyph {
            state: self.state.lock(),
        })
    }

    fn fill(&self, character: char) -> bool {
        let kind = u64::from(self.has_scalable());
        let key = (kind << 48) | (u64::from(self.size) << 32) | u64::from(character as u32);
        let mut state = self.state.lock();
        if let Some(cached) = cache::get(key) {
            for row in 0..cached.height {
                let start = row * MAX_GLYPH;
                state.buffer[start..start + cached.width]
                    .copy_from_slice(&cached.buffer[start..start + cached.width]);
            }
            state.width = cached.width;
            state.height = cached.height;
            state.left = cached.left;
            state.top = cached.top;
            state.advance = cached.advance;
            return true;
        }
        state.reset();
        let filled =
            self.fill_scalable(character, &mut state) || self.fill_bitmap(character, &mut state);
        if filled {
            cache::put(key, &state);
        }
        filled
    }

    /// 用可缩放字形填充状态；没有可缩放字形或栅格化失败时返回 `false`。
    #[cfg(feature = "scalable")]
    fn fill_scalable(&self, character: char, state: &mut State) -> bool {
        let Some(face) = self.face.as_ref() else {
            return false;
        };
        let Some(id) = face.glyph_index(character) else {
            return false;
        };
        state.advance = face
            .glyph_hor_advance(id)
            .map(|advance| libm::roundf(f32::from(advance) * self.scale) as i32)
            .unwrap_or(self.size as i32 / 2)
            .max(1);
        rasterize(face, id, self.scale, self.ascent, state)
    }

    /// 用可缩放字形填充状态；关闭 `scalable` 时恒为 `false`。
    #[cfg(not(feature = "scalable"))]
    fn fill_scalable(&self, _: char, _: &mut State) -> bool {
        false
    }

    fn fill_bitmap(&self, character: char, state: &mut State) -> bool {
        let Some(source) = self.fallback.glyph(character) else {
            state.advance = (self.size as i32 / 2).max(1);
            return character == ' ';
        };
        let source_advance = source.metric().xadvance.max(0) as usize;
        let source_width = if source_advance > 8 { 16 } else { 8 };
        let mut mask = [false; 16 * 16];
        source.for_each_pixel(|x, y| {
            if x >= 0 && y >= 0 && (x as usize) < source_width && (y as usize) < 16 {
                mask[y as usize * source_width + x as usize] = true;
            }
        });
        let target_width = (source_width * self.size as usize / 16).max(1);
        let target_height = (16 * self.size as usize / 16).max(1);
        if target_width > MAX_GLYPH || target_height > MAX_GLYPH {
            return false;
        }
        for row in 0..target_height {
            state.buffer[row * MAX_GLYPH..row * MAX_GLYPH + target_width].fill(0);
            let source_y = (row * source_width / target_width).min(15);
            for column in 0..target_width {
                let source_x = (column * source_width / target_width).min(source_width - 1);
                if mask[source_y * source_width + source_x] {
                    state.buffer[row * MAX_GLYPH + column] = 255;
                }
            }
        }
        state.width = target_width;
        state.height = target_height;
        state.left = 0;
        state.top = self.ascent - target_height as i32;
        state.advance = (source_advance * self.size as usize / 16).max(1) as i32;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "scalable")]
    #[test]
    fn rasterizes_latin_glyph() {
        let font = Font::new(16);
        assert!(font.has_scalable());
        let glyph = font.glyph('A').expect("A");
        assert!(glyph.metric().xadvance > 0);
        let mut pixels = 0;
        glyph.for_each_pixel(|_, _| pixels += 1);
        assert!(pixels > 10);
    }

    #[test]
    fn falls_back_for_cjk() {
        let font = Font::new(16);
        let glyph = font.glyph('中').expect("中");
        assert!(glyph.metric().xadvance >= 16);
        let mut pixels = 0;
        glyph.for_each_pixel(|_, _| pixels += 1);
        assert!(pixels > 10);
    }

    #[test]
    fn scales_with_size() {
        let small = Font::new(16);
        let large = Font::new(32);
        assert!(large.ascent() > small.ascent());
        let glyph = large.glyph('A').expect("A");
        assert!(glyph.metric().xadvance > small.glyph('A').unwrap().metric().xadvance);
    }

    #[test]
    fn monospace_advances_on_bitmap_grid() {
        let font = Font::monospace(16);
        assert!(!font.has_scalable());
        assert_eq!(font.advance('A'), 8);
        assert_eq!(font.advance('中'), 16);
        let glyph = font.glyph('中').expect("中");
        assert_eq!(glyph.width(), 16);
        assert_eq!(glyph.height(), 16);
    }

    #[test]
    fn coverage_matches_glyph_size() {
        let font = Font::new(16);
        let glyph = font.glyph('A').expect("A");
        let coverage = glyph.coverage();
        let covered = (0..glyph.height()).any(|row| {
            let start = row * glyph.stride();
            coverage[start..start + glyph.width()]
                .iter()
                .any(|value| *value > 0)
        });
        assert!(covered);
        assert!(coverage.len() >= glyph.height() * glyph.stride());
    }

    #[test]
    fn monospace_grid_matches_cell() {
        let font = Font::monospace(16);
        assert_eq!(font.max_width(), 16);
        assert_eq!(font.max_height(), 16);
        for character in [' ', 'A', '中', '?'] {
            assert!(font.has_glyph(character), "{character} 应有字形");
        }
    }
}
