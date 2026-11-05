//! 可缩放字体：TrueType 解析、轮廓栅格化与 8×16 位图回退。
//!
//! 内置 Adwaita Sans（SIL OFL 1.1）；缺少字形时回退到 `efont_cn_16` 位图
//! 字体，并按目标字号缩放。栅格化使用固定缓冲与 4×4 超采样，无堆分配。

#![no_std]

#[cfg(test)]
extern crate std;

use core::cell::{Ref, RefCell};

use lovyangfx_fonts::Font as BitmapFont;
use lovyangfx_fonts::fonts::efont_cn_16;
use spin::Mutex;
use ttf_parser::{Face, OutlineBuilder};

/// 内置可缩放字体。
const FONT_DATA: &[u8] = include_bytes!("../assets/AdwaitaSans-Regular.ttf");
/// 单字形位图边长上限。
const MAX_GLYPH: usize = 48;
/// 轮廓线段上限。
const MAX_EDGES: usize = 512;
/// 超采样倍数。
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
    next: usize,
}

impl Cache {
    const fn new() -> Self {
        Self {
            entries: [Cached::EMPTY; CACHE_SLOTS],
            next: 0,
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
    state: Ref<'a, State>,
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
        self.buffer.fill(0);
        self.width = 0;
        self.height = 0;
        self.left = 0;
        self.top = 0;
        self.advance = 0;
    }
}

fn cache_get(key: u64) -> Option<Cached> {
    CACHE
        .lock()
        .entries
        .iter()
        .find(|entry| entry.key == key)
        .copied()
}

fn cache_put(key: u64, state: &State) {
    let mut cache = CACHE.lock();
    let slot = cache.next % CACHE_SLOTS;
    cache.next = cache.next.wrapping_add(1);
    cache.entries[slot] = Cached {
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
    face: Option<Face<'static>>,
    fallback: BitmapFont<'static>,
    size: u32,
    scale: f32,
    ascent: i32,
    line_height: i32,
    state: RefCell<State>,
}

impl Font {
    /// 以像素字号构造字体。
    pub fn new(size: u32) -> Self {
        let size = size.max(4);
        let face = Face::parse(FONT_DATA, 0).ok();
        let (scale, ascent, line_height) = match face.as_ref() {
            Some(face) if face.units_per_em() > 0 => {
                let scale = size as f32 / f32::from(face.units_per_em());
                let ascent = ceil(f32::from(face.ascender()) * scale) as i32;
                let height =
                    ceil(f32::from(face.ascender() - face.descender() + face.line_gap()) * scale)
                        as i32;
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
            state: RefCell::new(State::new()),
        }
    }

    /// 是否成功加载可缩放字体。
    pub fn has_scalable(&self) -> bool {
        self.face.is_some() && self.scale > 0.0
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

    /// 计算字符前进宽度，不栅格化字形。
    pub fn advance(&self, character: char) -> i32 {
        if let Some(face) = self.face.as_ref()
            && let Some(id) = face.glyph_index(character)
            && let Some(advance) = face.glyph_hor_advance(id)
        {
            return (round(f32::from(advance) * self.scale) as i32).max(1);
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
            state: self.state.borrow(),
        })
    }

    fn fill(&self, character: char) -> bool {
        let key = (u64::from(self.size) << 32) | u64::from(character as u32);
        let mut state = self.state.borrow_mut();
        if let Some(cached) = cache_get(key) {
            state.buffer = cached.buffer;
            state.width = cached.width;
            state.height = cached.height;
            state.left = cached.left;
            state.top = cached.top;
            state.advance = cached.advance;
            return true;
        }
        state.reset();
        let filled = if let Some(face) = self.face.as_ref()
            && let Some(id) = face.glyph_index(character)
        {
            state.advance = face
                .glyph_hor_advance(id)
                .map(|advance| round(f32::from(advance) * self.scale) as i32)
                .unwrap_or(self.size as i32 / 2)
                .max(1);
            rasterize(face, id, self.scale, self.ascent, &mut state)
                || self.fill_bitmap(character, &mut state)
        } else {
            self.fill_bitmap(character, &mut state)
        };
        if filled {
            cache_put(key, &state);
        }
        filled
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

struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Edge {
    const ZERO: Self = Self {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    };
}

struct Builder {
    edges: [Edge; MAX_EDGES],
    count: usize,
    current: (f32, f32),
    start: (f32, f32),
    scale: f32,
}

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
    let left = floor(min_x);
    let top = floor(min_y);
    let width = (ceil(max_x) - left).max(0.0) as usize;
    let height = (ceil(max_y) - top).max(0.0) as usize;
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

fn cross(edge: &Edge, x: f32, y: f32) -> f32 {
    (edge.x1 - edge.x0) * (y - edge.y0) - (x - edge.x0) * (edge.y1 - edge.y0)
}

fn lerp(from: (f32, f32), to: (f32, f32), t: f32) -> (f32, f32) {
    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
}

fn floor(value: f32) -> f32 {
    let integer = value as i32 as f32;
    if integer > value {
        integer - 1.0
    } else {
        integer
    }
}

fn ceil(value: f32) -> f32 {
    let integer = value as i32 as f32;
    if integer < value {
        integer + 1.0
    } else {
        integer
    }
}

fn round(value: f32) -> f32 {
    floor(value + 0.5)
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
}
