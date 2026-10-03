//! 颜色距离：RGB → CIELAB，再算 ΔE（CIE76，就是两个 Lab 点的直线距离）。dco 的 `class_de` 用的是同一种量法，
//! 所以配置里的「不是糖」的颜色，和玩的时候 dco 读回来的类别颜色，可以放在一起比。纯函数。

fn lin(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn rgb_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    // sRGB → XYZ（D65）
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (a, b) = (rgb_to_lab(a), rgb_to_lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// 一个给人看的颜色名（只用来在 `dct game look` 里打印，AI 对着截图核对时好认）。
pub fn colour_name(rgb: [u8; 3]) -> &'static str {
    let (r, g, b) = (rgb[0] as f64 / 255.0, rgb[1] as f64 / 255.0, rgb[2] as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let (v, d) = (max, max - min);
    let s = if max == 0.0 { 0.0 } else { d / max };
    if s < 0.15 {
        return if v > 0.85 {
            "白"
        } else if v < 0.25 {
            "黑"
        } else {
            "灰"
        };
    }
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    match h {
        h if !(15.0..345.0).contains(&h) => "红",
        h if h < 40.0 => {
            if v < 0.82 && s < 0.6 {
                "棕"
            } else {
                "橙"
            }
        }
        h if h < 70.0 => "黄",
        h if h < 170.0 => "绿",
        h if h < 200.0 => "青",
        h if h < 255.0 => {
            if s < 0.4 && v > 0.8 {
                "浅蓝"
            } else {
                "蓝"
            }
        }
        h if h < 300.0 => "紫",
        _ => "粉",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_colours_are_zero_apart_and_distance_is_symmetric() {
        assert_eq!(delta_e([10, 20, 30], [10, 20, 30]), 0.0);
        let (a, b) = ([200, 40, 40], [40, 200, 40]);
        assert!((delta_e(a, b) - delta_e(b, a)).abs() < 1e-9);
    }

    #[test]
    fn known_lab_values() {
        let w = rgb_to_lab([255, 255, 255]);
        assert!((w[0] - 100.0).abs() < 0.1 && w[1].abs() < 0.1 && w[2].abs() < 0.1, "{w:?}");
        let k = rgb_to_lab([0, 0, 0]);
        assert!(k[0].abs() < 0.1, "{k:?}");
        let red = rgb_to_lab([255, 0, 0]);
        assert!((red[0] - 53.24).abs() < 0.2 && (red[1] - 80.09).abs() < 0.3 && (red[2] - 67.2).abs() < 0.3, "{red:?}");
    }

    /// 真机第 1713 关读出来的七个类别：蜂蜜块（196,148,101）和橙色糖、黄糖要分得开（> 24），小偏差要认得出（< 24）。
    #[test]
    fn the_real_honey_colour_is_far_from_every_candy_but_close_to_itself() {
        let honey = [196, 148, 101];
        for candy in [[66, 102, 252], [174, 42, 253], [233, 68, 40], [114, 232, 24], [250, 231, 42], [161, 180, 233]] {
            assert!(delta_e(honey, candy) > 24.0, "{candy:?} 离蜂蜜块太近：{}", delta_e(honey, candy));
        }
        assert!(delta_e(honey, [199, 150, 104]) < 24.0);
        assert!(delta_e(honey, [190, 144, 98]) < 24.0);
    }

    #[test]
    fn colour_names_for_the_real_classes() {
        for (rgb, want) in [
            ([66, 102, 252], "蓝"),
            ([174, 42, 253], "紫"),
            ([233, 68, 40], "红"),
            ([196, 148, 101], "棕"),
            ([161, 180, 233], "浅蓝"),
            ([114, 232, 24], "绿"),
            ([250, 231, 42], "黄"),
            ([240, 149, 36], "橙"),
            ([236, 235, 234], "白"),
            ([128, 128, 128], "灰"),
        ] {
            assert_eq!(colour_name(rgb), want, "{rgb:?}");
        }
    }
}
