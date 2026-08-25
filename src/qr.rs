//! 把一段文字变成终端里画得出来的二维码。
//!
//! 用处只有一个：手机端的地址（带令牌）没法让人手输——64 个十六进制字符，
//! 输错一个字符的结果是 401，而用户完全不知道自己错在哪。二维码是这件事的
//! 必需品，不是装饰。
//!
//! ## 为什么每一格都自己钉死前景和背景色
//!
//! 二维码是给**摄像头**看的，不是给人看的，而摄像头认的是深浅对比。
//! 如果只画字符、让终端自己配色，那么在深色主题下整块码会变成"浅底深字"的
//! 反相版本——大部分手机能认反相码，但不是所有；更糟的是某些主题会把两档
//! 颜色调得很接近，扫出来是一团糊。
//!
//! 所以这里每一格都显式给一对色：深模块用 16 号（纯黑），浅模块用 231 号
//! （纯白）。**走 256 色索引而不是具名色**，理由跟 `BarTheme` 那条守卫一样：
//! 0–15 号会被终端主题改写，而这里最不能被改写的就是黑和白。
//!
//! ## 半块字符
//!
//! 终端的字符格大约是"高是宽的两倍"，直接一格一个模块画出来的码是被纵向
//! 拉长的，扫不出来。所以一个字符格装**上下两个模块**：用 `▀`（上半块），
//! 前景色画上面那个模块，背景色画下面那个——这样模块就接近正方形。

use qrcode::{EcLevel, QrCode};
use ratatui::style::Color;

/// 深模块的颜色（纯黑，256 色立方里的 16 号）。
pub const DARK: Color = Color::Indexed(16);
/// 浅模块的颜色（纯白，231 号）。
pub const LIGHT: Color = Color::Indexed(231);

/// 码周围必须留的空白模块数。**这不是留白好看**：解码器靠这一圈静区
/// 找到码的边界，少了它很多手机根本认不出来。规范要求 4 个模块。
const QUIET: usize = 4;

/// 终端上的一行：每个字符格是「上模块颜色 + 下模块颜色」。
///
/// 画的时候用 `▀`：前景色就是上半格，背景色就是下半格。
pub struct QrCell {
    pub top: Color,
    pub bottom: Color,
}

/// 一整块码，按行给出。行数已经把上下两个模块合成一格算过了。
pub struct QrArt {
    pub rows: Vec<Vec<QrCell>>,
    /// 一行有多少个字符格（含静区）。调用方要拿它算摆得下摆不下。
    pub cols: usize,
}

/// 把 `data` 编成二维码，再排成终端能画的格子。
///
/// 纠错等级用 `L`（最低）：这个码是在**屏幕上**给旁边的手机扫的，没有印刷、
/// 折角、油污那些问题，而更高的纠错等级意味着更多模块、更大的码——在一个
/// 80×24 的终端里，那是"放不放得下"的差别。
///
/// 编不出来（数据太长）返回 `None`。调用方那时候该退回"把网址写出来"，
/// 而不是显示一块坏图。
pub fn render(data: &str) -> Option<QrArt> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L).ok()?;
    let modules = code.to_colors();
    let side = (modules.len() as f64).sqrt() as usize;
    if side * side != modules.len() {
        return None;
    }

    let dark = |x: usize, y: usize| -> bool {
        // 静区之外一律当浅色。这样下面的循环不用为边界写特例。
        if x < QUIET || y < QUIET || x >= side + QUIET || y >= side + QUIET {
            return false;
        }
        modules[(y - QUIET) * side + (x - QUIET)] == qrcode::Color::Dark
    };

    let cols = side + QUIET * 2;
    let full = cols; // 含静区之后是个正方形
    let mut rows = Vec::new();
    // 一格装上下两个模块，所以纵向每次跨两行。
    let mut y = 0;
    while y < full {
        let mut row = Vec::with_capacity(cols);
        for x in 0..cols {
            row.push(QrCell {
                top: if dark(x, y) { DARK } else { LIGHT },
                // 最后一行可能没有"下半个"模块——那就当浅色，正好接上静区。
                bottom: if y + 1 < full && dark(x, y + 1) {
                    DARK
                } else {
                    LIGHT
                },
            });
        }
        rows.push(row);
        y += 2;
    }
    Some(QrArt { rows, cols })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把画出来的格子还原成一张位图（一格两行），交给一个**真的解码器**去读。
    ///
    /// 这是这份代码唯一诚实的验收方式：`render` 自己认为画对了不算数，
    /// 手机上的解码器认得出来才算。用眼睛看一块黑白点阵看不出任何东西。
    fn decode(art: &QrArt) -> String {
        // 每个模块放大成 4×4 像素：解码器要在图里找定位图案，一个模块一个
        // 像素的图它认不出来——那不是码的问题，是采样率的问题。
        const SCALE: usize = 4;
        let w = art.cols * SCALE;
        let h = art.rows.len() * 2 * SCALE;
        let dark_at = |x: usize, y: usize| -> bool {
            let cell_row = y / SCALE / 2;
            let half = (y / SCALE) % 2;
            let cell_col = x / SCALE;
            let cell = &art.rows[cell_row][cell_col];
            let c = if half == 0 { cell.top } else { cell.bottom };
            c == DARK
        };
        let mut prepared = rqrr::PreparedImage::prepare_from_bitmap(w, h, dark_at);
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 1, "解码器没找到（或者找到了不止一个）码");
        grids[0].decode().expect("码解不开").1
    }

    /// **真的能扫出来吗。** 一个真实的手机端地址，画出来，再用解码器读回去，
    /// 必须一字不差。
    #[test]
    fn a_rendered_code_decodes_back_to_the_same_url() {
        let url = "http://192.168.1.19:53114/#t=3bcba325dfc310f4297be42171bae339bc842339daae473b3db49166ff7b550c";
        let art = render(url).expect("这段长度该编得出来");
        assert_eq!(decode(&art), url);
    }

    /// 静区不能少。少了它，很多解码器直接找不到码——而「在我的手机上能扫」
    /// 不代表在别人的手机上能扫。
    ///
    /// **这里的 4 和 2 是写死的，故意不引用 `QUIET`。** 拿常量去算循环边界的话，
    /// 把 `QUIET` 改成 0 之后循环变成空转，测试照样全绿——这不是假想，
    /// 是变异测试当场抓到的：静区去掉和减半两种改法都从这条测试底下溜过去了。
    #[test]
    fn the_quiet_zone_is_four_modules_on_every_side() {
        // 规范要求 4 个模块；一个字符格装 2 个模块，所以是 2 个字符行。
        const MODULES: usize = 4;
        const CHAR_ROWS: usize = 2;

        let art = render("http://x/#t=1").unwrap();
        let light = |c: &QrCell| c.top == LIGHT && c.bottom == LIGHT;

        for r in 0..CHAR_ROWS {
            assert!(art.rows[r].iter().all(light), "上边第 {r} 行不是静区");
            let from_bottom = art.rows.len() - 1 - r;
            assert!(
                art.rows[from_bottom].iter().all(light),
                "下边第 {r} 行不是静区"
            );
        }
        for row in art.rows.iter() {
            for c in 0..MODULES {
                assert!(light(&row[c]), "左边第 {c} 列不是静区");
                assert!(light(&row[art.cols - 1 - c]), "右边第 {c} 列不是静区");
            }
        }

        // **静区正好这么宽，不多不少。** 紧挨着它的就该是左上角那个定位图案——
        // 没有这一条，一个把整块码都涂成浅色的实现也能过上面那些断言。
        assert!(
            !light(&art.rows[CHAR_ROWS][MODULES]),
            "静区之后该马上是定位图案，实际还是浅色——码本身画丢了"
        );
    }

    /// **两种颜色都必须是 256 色索引，不能是具名色。** 0–15 号会被终端主题
    /// 改写，而这里最不能被改写的就是黑和白——改写的结果是一块扫不出来的码。
    /// 同 `BarTheme` 那条守卫。
    #[test]
    fn the_two_colours_cannot_be_repainted_by_a_terminal_theme() {
        for c in [DARK, LIGHT] {
            match c {
                Color::Indexed(i) => assert!(i >= 16, "{i} 号色会被主题改写"),
                other => panic!("必须是 256 色索引，实际 {other:?}"),
            }
        }
        assert_ne!(DARK, LIGHT);
    }

    /// 编不出来就说编不出来，不画一块坏图。
    #[test]
    fn data_that_cannot_be_encoded_returns_none() {
        // L 级最多约 2953 字节，给它一个远超上限的。
        assert!(render(&"x".repeat(5000)).is_none());
    }

    /// 一格两行，所以行数是「边长（含静区）向上取整除以 2」。
    /// 这条钉的是最后一行那个"下半格没有模块"的边界——写成向下取整的话，
    /// 边长是奇数时最后一行模块整排丢掉，而码照样"看起来像个码"。
    #[test]
    fn every_module_row_survives_the_halving() {
        let art = render("http://x/#t=1").unwrap();
        let side = art.cols; // 含静区是正方形
        assert_eq!(
            art.rows.len(),
            side.div_ceil(2),
            "行数不对，底下那排模块丢了"
        );
    }
}
