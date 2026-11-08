//! TrueType 轮廓栅格化：线段收集与 4×4 超采样扫描。

use ttf_parser::{Face, OutlineBuilder};

use crate::glyph::{MAX_GLYPH, State};

/// 轮廓线段上限。
const MAX_EDGES: usize = 512;
/// 超采样倍数。
const SAMPLES: u32 = 4;

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

/// 把字形轮廓栅格化到状态缓冲；尺寸超限或字形为空时返回 `false`/`true`。
pub(crate) fn rasterize(
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
