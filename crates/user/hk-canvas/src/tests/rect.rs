//! 矩形集合运算测试。

use crate::Rect;

#[test]
fn intersect_narrows_to_overlap() {
    let area = Rect::new(0, 0, 10, 10).intersect(Rect::new(5, 5, 10, 10));
    assert_eq!(area, Rect::new(5, 5, 5, 5));
}

#[test]
fn intersect_without_overlap_is_empty() {
    let area = Rect::new(0, 0, 4, 4).intersect(Rect::new(8, 8, 4, 4));
    assert!(area.is_empty());
    assert_eq!(area.width, 0);
    assert_eq!(area.height, 0);
}

#[test]
fn union_bounds_both_rectangles() {
    assert_eq!(
        Rect::new(0, 0, 4, 4).union(Rect::new(6, 2, 2, 2)),
        Rect::new(0, 0, 8, 4)
    );
    assert_eq!(
        Rect::new(0, 0, 0, 0).union(Rect::new(1, 1, 2, 2)),
        Rect::new(1, 1, 2, 2)
    );
}

#[test]
fn translate_and_inflate_keep_size_semantics() {
    assert_eq!(
        Rect::new(2, 3, 4, 5).translate(-1, 2),
        Rect::new(1, 5, 4, 5)
    );
    assert_eq!(Rect::new(2, 3, 4, 5).inflate(1, 2), Rect::new(1, 1, 6, 9));
    assert_eq!(Rect::new(2, 3, 4, 5).inflate(-1, -2), Rect::new(3, 5, 2, 1));
}

#[test]
fn contains_uses_half_open_bounds() {
    let rect = Rect::new(1, 2, 3, 4);
    assert!(rect.contains(1, 2));
    assert!(rect.contains(3, 5));
    assert!(!rect.contains(4, 5));
    assert!(!rect.contains(1, 6));
}
