//! 字体：TrueType 解析、轮廓栅格化、等宽位图与 8×16 回退。
//!
//! 默认构建包含可缩放字体：内置 Adwaita Sans（SIL OFL 1.1），缺少字形时回退到
//! `efont_cn_16` 位图字体并按目标字号缩放，栅格化使用固定缓冲与 4×4 超采样。
//! 关闭 `scalable` 特性后只保留位图字体，用于不需要 TrueType 的早期启动链路。
//!
//! [`default_font`] 提供首次使用时初始化并常驻的 16 像素字体，供不指定字体的通用排版复用。

#![no_std]

#[cfg(test)]
extern crate std;

use lovyangfx_fonts::Font as BitmapFont;
use lovyangfx_fonts::fonts::efont_cn_16;
use spin::{Mutex, MutexGuard, Once};
#[cfg(feature = "scalable")]
use ttf_parser::{Face, OutlineBuilder};

/// 可缩放字体的字形类型；关闭 `scalable` 时没有可缩放字形。
#[cfg(feature = "scalable")]
type FaceSlot = Face<'static>;
/// 可缩放字体的字形类型；关闭 `scalable` 时没有可缩放字形。
#[cfg(not(feature = "scalable"))]
type FaceSlot = ();

/// 内置可缩放字体。
#[cfg(feature = "scalable")]
const FONT_DATA: &[u8] = include_bytes!("../assets/AdwaitaSans-Regular.ttf");
/// 单字形位图边长上限。
const MAX_GLYPH: usize = 48;
/// 轮廓线段上限。
#[cfg(feature = "scalable")]
const MAX_EDGES: usize = 512;
/// 超采样倍数。
#[cfg(feature = "scalable")]
const SAMPLES: u32 = 4;
/// 字形缓存条目数。
const CACHE_SLOTS: usize = 128;

#[derive(Clone, Copy)]
struct Cached {
    key: u64,
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    advance: i32,
    buffer: [u8; MAX_GLYPH * MAX_GLYPH],
}

impl Cached {
    const EMPTY: Self = Self {
        key: 0,
        width: 0,
        height: 0,
        left: 0,
        top: 0,
        advance: 0,
        buffer: [0; MAX_GLYPH * MAX_GLYPH],
    };
}

struct Cache {
    entries: [Cached; CACHE_SLOTS],
}

impl Cache {
    const fn new() -> Self {
        Self {
            entries: [Cached::EMPTY; CACHE_SLOTS],
        }
    }
}

static CACHE: Mutex<Cache> = Mutex::new(Cache::new());

/// 字形度量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    /// 水平前进宽度（像素）。
    pub xadvance: i32,
}

/// 已栅格化的字形。
pub struct Glyph<'a> {
    state: MutexGuard<'a, State>,
}

impl Glyph<'_> {
    /// 遍历覆盖像素，坐标相对单元格左上角。
    pub fn for_each_pixel(&self, mut draw: impl FnMut(i32, i32)) {
        for row in 0..self.state.height {
            for column in 0..self.state.width {
                if self.state.buffer[row * MAX_GLYPH + column] >= 96 {
                    draw(self.state.left + column as i32, self.state.top + row as i32);
                }
            }
        }
    }

    /// 返回度量。
    pub fn metric(&self) -> Metric {
        Metric {
            xadvance: self.state.advance,
        }
    }

    /// 位图宽度（像素）。
    pub fn width(&self) -> usize {
        self.state.width
    }

    /// 位图高度（像素）。
    pub fn height(&self) -> usize {
        self.state.height
    }

    /// 位图左边相对绘制原点的偏移。
    pub fn left(&self) -> i32 {
        self.state.left
    }

    /// 位图顶边相对绘制原点的偏移。
    pub fn top(&self) -> i32 {
        self.state.top
    }

    /// 覆盖率数据的行跨度。
    pub fn stride(&self) -> usize {
        MAX_GLYPH
    }

    /// 覆盖率数据：0 为完全透明，255 为完全覆盖。
    pub fn coverage(&self) -> &[u8] {
        &self.state.buffer[..]
    }
}

struct State {
    buffer: [u8; MAX_GLYPH * MAX_GLYPH],
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    advance: i32,
}

impl State {
    const fn new() -> Self {
        Self {
            buffer: [0; MAX_GLYPH * MAX_GLYPH],
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            advance: 0,
        }
    }

    fn reset(&mut self) {
        self.width = 0;
        self.height = 0;
        self.left = 0;
        self.top = 0;
        self.advance = 0;
    }
}

/// 返回缓存 key 的起始槽位；槽位数是 2 的幂。
fn cache_slot(key: u64) -> usize {
    let hash = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((hash >> 33) as usize) & (CACHE_SLOTS - 1)
}

fn cache_get(key: u64) -> Option<Cached> {
    let cache = CACHE.lock();
    let mut index = cache_slot(key);
    for _ in 0..CACHE_SLOTS {
        let entry = cache.entries[index];
        if entry.key == key {
            return Some(entry);
        }
        if entry.key == 0 {
            return None;
        }
        index = (index + 1) & (CACHE_SLOTS - 1);
    }
    None
}

fn cache_put(key: u64, state: &State) {
    let mut cache = CACHE.lock();
    let mut index = cache_slot(key);
    for _ in 0..CACHE_SLOTS {
        if cache.entries[index].key == key || cache.entries[index].key == 0 {
            break;
        }
        index = (index + 1) & (CACHE_SLOTS - 1);
    }
    cache.entries[index] = Cached {
        key,
        width: state.width,
        height: state.height,
        left: state.left,
        top: state.top,
        advance: state.advance,
        buffer: state.buffer,
    };
}

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

/// 返回内置默认字体：16 像素可缩放字体，缺字形时回退到内置位图字体。
///
/// 所有调用共享同一份字形缓存，适合不指定字体的通用文本绘制。
pub fn default_font() -> &'static Font {
    DEFAULT.call_once(|| Font::new(16))
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
                fallback: efont_cn_16(),
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
            fallback: efont_cn_16(),
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
        if let Some(cached) = cache_get(key) {
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
            cache_put(key, &state);
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

#[cfg(feature = "scalable")]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

#[cfg(feature = "scalable")]
impl Edge {
    const ZERO: Self = Self {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    };
}

#[cfg(feature = "scalable")]
struct Builder {
    edges: [Edge; MAX_EDGES],
    count: usize,
    current: (f32, f32),
    start: (f32, f32),
    scale: f32,
}

#[cfg(feature = "scalable")]
impl Builder {
    fn new(scale: f32) -> Self {
        Self {
            edges: [Edge::ZERO; MAX_EDGES],
            count: 0,
            current: (0.0, 0.0),
            start: (0.0, 0.0),
            scale,
        }
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale, -y * self.scale)
    }

    fn push(&mut self, from: (f32, f32), to: (f32, f32)) {
        if self.count >= MAX_EDGES || (from.0 == to.0 && from.1 == to.1) {
            return;
        }
        self.edges[self.count] = Edge {
            x0: from.0,
            y0: from.1,
            x1: to.0,
            y1: to.1,
        };
        self.count += 1;
    }
}

#[cfg(feature = "scalable")]
impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.current = self.point(x, y);
        self.start = self.current;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let next = self.point(x, y);
        self.push(self.current, next);
        self.current = next;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let from = self.current;
        let control = self.point(x1, y1);
        let to = self.point(x, y);
        let mut previous = from;
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            let point = lerp(lerp(from, control, t), lerp(control, to, t), t);
            self.push(previous, point);
            previous = point;
        }
        self.current = to;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let from = self.current;
        let c1 = self.point(x1, y1);
        let c2 = self.point(x2, y2);
        let to = self.point(x, y);
        let mut previous = from;
        for step in 1..=12 {
            let t = step as f32 / 12.0;
            let inverse = 1.0 - t;
            let point = (
                inverse * inverse * inverse * from.0
                    + 3.0 * inverse * inverse * t * c1.0
                    + 3.0 * inverse * t * t * c2.0
                    + t * t * t * to.0,
                inverse * inverse * inverse * from.1
                    + 3.0 * inverse * inverse * t * c1.1
                    + 3.0 * inverse * t * t * c2.1
                    + t * t * t * to.1,
            );
            self.push(previous, point);
            previous = point;
        }
        self.current = to;
    }

    fn close(&mut self) {
        if self.current != self.start {
            self.push(self.current, self.start);
            self.current = self.start;
        }
    }
}

#[cfg(feature = "scalable")]
fn rasterize(
    face: &Face<'_>,
    id: ttf_parser::GlyphId,
    scale: f32,
    ascent: i32,
    state: &mut State,
) -> bool {
    let mut builder = Builder::new(scale);
    if face.outline_glyph(id, &mut builder).is_none() {
        return true;
    }
    if builder.count == 0 {
        return true;
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for edge in &builder.edges[..builder.count] {
        min_x = min_x.min(edge.x0.min(edge.x1));
        min_y = min_y.min(edge.y0.min(edge.y1));
        max_x = max_x.max(edge.x0.max(edge.x1));
        max_y = max_y.max(edge.y0.max(edge.y1));
    }
    let left = libm::floorf(min_x);
    let top = libm::floorf(min_y);
    let width = (libm::ceilf(max_x) - left).max(0.0) as usize;
    let height = (libm::ceilf(max_y) - top).max(0.0) as usize;
    if width == 0 || height == 0 {
        return true;
    }
    if width > MAX_GLYPH || height > MAX_GLYPH {
        return false;
    }
    let edges = &builder.edges[..builder.count];
    for row in 0..height {
        for column in 0..width {
            let mut hits = 0;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let x = left + column as f32 + (sample_x as f32 + 0.5) / SAMPLES as f32;
                    let y = top + row as f32 + (sample_y as f32 + 0.5) / SAMPLES as f32;
                    if winding(edges, x, y) != 0 {
                        hits += 1;
                    }
                }
            }
            state.buffer[row * MAX_GLYPH + column] = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
        }
    }
    state.width = width;
    state.height = height;
    state.left = left as i32;
    state.top = ascent + top as i32;
    true
}

#[cfg(feature = "scalable")]
fn winding(edges: &[Edge], x: f32, y: f32) -> i32 {
    let mut winding = 0;
    for edge in edges {
        if edge.y0 <= y {
            if edge.y1 > y && cross(edge, x, y) > 0.0 {
                winding += 1;
            }
        } else if edge.y1 <= y && cross(edge, x, y) < 0.0 {
            winding -= 1;
        }
    }
    winding
}

#[cfg(feature = "scalable")]
fn cross(edge: &Edge, x: f32, y: f32) -> f32 {
    (edge.x1 - edge.x0) * (y - edge.y0) - (x - edge.x0) * (edge.y1 - edge.y0)
}

#[cfg(feature = "scalable")]
fn lerp(from: (f32, f32), to: (f32, f32), t: f32) -> (f32, f32) {
    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

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
