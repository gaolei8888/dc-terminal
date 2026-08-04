//! 九宫格的布局数学。全是纯函数，跟终端、协议、会话都没关系——
//! 能独立测，也只在这里测。
//!
//! 这些函数是 Task 5（视图接线）的接口，本任务里还没有调用方——
//! `mod grid;` 是私有的，函数体只在自己的测试里跑，所以 clippy 会把
//! 它们当成死代码。等 Task 5 接上视图就不需要这条 allow 了。
#![allow(dead_code)]

use super::widgets::char_width;
use crate::pty::ScreenSpan;

pub const TILES_PER_PAGE: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// 当页格子数 → （行数，列数）。上限九格；空看板画一个空格子占位，
/// 免得渲染分支到处判零。
pub fn grid_shape(count: usize) -> (u16, u16) {
    match count {
        0 | 1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        5 | 6 => (2, 3),
        _ => (3, 3),
    }
}

pub fn page_of(focus: usize) -> usize {
    focus / TILES_PER_PAGE
}

pub fn page_count(total: usize) -> usize {
    if total == 0 {
        1
    } else {
        total.div_ceil(TILES_PER_PAGE)
    }
}

/// 焦点在格子间移动。左右在全体会话上一维回绕（越过页边自然翻页）；
/// 上下在当页的二维布局里走，向下越出最后一行收到最后一格。
pub fn move_focus(focus: usize, total: usize, dir: Dir) -> usize {
    if total == 0 {
        return 0;
    }
    let page_start = page_of(focus) * TILES_PER_PAGE;
    let in_page = focus - page_start;
    let page_len = (total - page_start).min(TILES_PER_PAGE);
    let (_, cols) = grid_shape(page_len);
    let cols = cols as usize;
    match dir {
        Dir::Right => (focus + 1) % total,
        Dir::Left => (focus + total - 1) % total,
        Dir::Down => {
            let down = in_page + cols;
            page_start + down.min(page_len - 1)
        }
        Dir::Up => page_start + in_page.saturating_sub(cols),
    }
}

/// 按显示宽度裁一行。宽字符（CJK 占两列）跨过边界就整个丢掉——
/// 裁一半会把后面所有列推歪。宽度的定义必须跟 widgets 里的
/// `char_width` 是同一份，两边悄悄分叉的话裁的位置就对不上。
pub fn crop_line(spans: &[ScreenSpan], max_cols: usize) -> Vec<ScreenSpan> {
    let mut out: Vec<ScreenSpan> = Vec::new();
    let mut used = 0usize;
    for sp in spans {
        if used >= max_cols {
            break;
        }
        let mut text = String::new();
        for ch in sp.text.chars() {
            let w = char_width(ch);
            if used + w > max_cols {
                break;
            }
            used += w;
            text.push(ch);
        }
        if !text.is_empty() {
            out.push(ScreenSpan {
                text,
                style: sp.style,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{ScreenSpan, ScreenStyle};

    #[test]
    fn shape_scales_with_session_count() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(2), (1, 2));
        assert_eq!(grid_shape(3), (2, 2));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(5), (2, 3));
        assert_eq!(grid_shape(6), (2, 3));
        assert_eq!(grid_shape(7), (3, 3));
        assert_eq!(grid_shape(9), (3, 3));
        // 超过 9 的调用方先按页切好再问形状，这里按满页算
        assert_eq!(grid_shape(0), (1, 1), "空看板画一个空格子占位");
    }

    #[test]
    fn paging_math() {
        assert_eq!(page_of(0), 0);
        assert_eq!(page_of(8), 0);
        assert_eq!(page_of(9), 1);
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(9), 1);
        assert_eq!(page_count(10), 2);
    }

    #[test]
    fn focus_moves_in_two_dimensions_and_wraps_pages() {
        // 5 个会话 → 2×3 布局，index 0..=4
        assert_eq!(move_focus(0, 5, Dir::Right), 1);
        assert_eq!(
            move_focus(2, 5, Dir::Down),
            4,
            "2 的正下方越出最后一行，收到最后一格"
        );
        assert_eq!(move_focus(0, 5, Dir::Down), 3);
        assert_eq!(move_focus(4, 5, Dir::Right), 0, "尾格右移回绕到头");
        assert_eq!(move_focus(0, 5, Dir::Left), 4, "头格左移回绕到尾");
        // 10 个会话：第 8 格（第一页尾）右移进第二页
        assert_eq!(move_focus(8, 10, Dir::Right), 9);
        assert_eq!(move_focus(9, 10, Dir::Right), 0);
    }

    fn sp(text: &str) -> ScreenSpan {
        ScreenSpan {
            text: text.into(),
            style: ScreenStyle::default(),
        }
    }

    #[test]
    fn crop_cuts_at_display_width_without_splitting_wide_chars() {
        // "干活中" 每个字占 2 列。上限 5 列 → 只装得下 2 个字（4 列），
        // 第 3 个字会跨过边界，整个丢掉。
        let out = crop_line(&[sp("干活中")], 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "干活");

        // 跨 span 累计：第一个 span 占 3 列，剩 2 列只够 "b" 一个
        let out = crop_line(&[sp("abc"), sp("bcd")], 5);
        assert_eq!(out[1].text, "bc");

        // 不超限的原样保留
        let out = crop_line(&[sp("ok")], 80);
        assert_eq!(out[0].text, "ok");
    }
}
