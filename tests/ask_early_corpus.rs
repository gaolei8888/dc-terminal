//! 早出首段切点质量：把 `fixtures/ask_early_corpus.txt` 里手写的短回复喂给 `ask::early::first_cut`，
//! 数「首段几个字、切不切得出、有没有丑切点」。语料是我自己写的（不是模型输出），分词标记也是我手标的，
//! 所以这是**自测**不是验收：`tune` 段调过参，`heldout` 段是调参前写好、调参时没看过结果的。
//! `cargo test --test ask_early_corpus -- --nocapture` 能看到完整表。
use dct::ask::early::{first_cut, EarlyConfig};
use dct::ask::split::SentenceSplitter;
use std::collections::BTreeSet;

struct Item {
    raw: Vec<char>,
    marks: BTreeSet<usize>,
    heldout: bool,
    adversarial: bool,
}

fn corpus() -> Vec<Item> {
    let (mut heldout, mut adversarial) = (false, false);
    let mut out = Vec::new();
    for line in include_str!("fixtures/ask_early_corpus.txt").lines() {
        match line {
            "# tune" => (heldout, adversarial) = (false, false),
            "# heldout" => (heldout, adversarial) = (true, false),
            "# adversarial" => (heldout, adversarial) = (false, true),
            l if l.starts_with('#') || l.trim().is_empty() => {}
            l => {
                let (mut raw, mut marks) = (Vec::new(), BTreeSet::new());
                for c in l.replace("\\n", "\n").chars() {
                    if c == '|' {
                        marks.insert(raw.len());
                    } else {
                        raw.push(c);
                    }
                }
                out.push(Item { raw, marks, heldout, adversarial });
            }
        }
    }
    out
}

/// 第一句在流里的样子：逐字喂 `SentenceSplitter`，第一句一出来就停；返回它**还在凑**时的
/// 最长前缀长度（不含收尾那个字）。流里一直没出句子就是整段（自然结束时的尾巴）。
fn pending_len(raw: &[char]) -> usize {
    let mut sp = SentenceSplitter::new();
    for (i, c) in raw.iter().enumerate() {
        if !sp.push(&c.to_string()).is_empty() {
            return i;
        }
    }
    raw.len()
}

#[derive(Default, Debug)]
struct Row {
    n: usize,
    cut: usize,
    /// 没切的里面：第一句本来就短（不到 n_min 个字）。
    too_short: usize,
    first_chars: Vec<usize>,
    saved_chars: Vec<usize>,
    half_word: Vec<String>,
    after_func: Vec<String>,
    before_particle: Vec<String>,
    inside_ascii: Vec<String>,
}

impl Row {
    fn ugly(&self) -> usize {
        self.half_word.len() + self.after_func.len() + self.before_particle.len() + self.inside_ascii.len()
    }
}

const JUDGE_FUNC: &str = "的了着过地得和与在把被将对比";
const JUDGE_PARTICLE: &str = "的了着过吗呢吧啊呀么嘛哦啦";

fn is_cjk(c: char) -> bool {
    c.is_alphanumeric() && !c.is_ascii()
}

/// `which`：0 = tune，1 = heldout，2 = adversarial（专门挑会咬人的词写的），None = tune + heldout。
fn run(cfg: &EarlyConfig, which: Option<u8>) -> Row {
    let mut row = Row::default();
    for it in corpus().iter().filter(|i| match which {
        None => !i.adversarial,
        Some(0) => !i.adversarial && !i.heldout,
        Some(1) => i.heldout,
        Some(_) => i.adversarial,
    }) {
        row.n += 1;
        let limit = pending_len(&it.raw);
        let mut at = None;
        for p in 1..=limit {
            let prefix: String = it.raw[..p].iter().collect();
            if let Some(b) = first_cut(&prefix, cfg) {
                at = Some(prefix[..b].chars().count());
                break;
            }
        }
        let Some(c) = at else {
            if it.raw[..limit].iter().filter(|c| c.is_alphanumeric()).count() <= cfg.n_min as usize {
                row.too_short += 1;
            }
            continue;
        };
        row.cut += 1;
        let (head, tail): (String, String) = (it.raw[..c].iter().collect(), it.raw[c..].iter().collect());
        let shown = format!("{head}｜{tail}");
        row.first_chars.push(head.chars().filter(|c| c.is_alphanumeric()).count());
        row.saved_chars.push(limit.saturating_sub(c));
        let (prev, next) = (it.raw[c - 1], it.raw[c]);
        if is_cjk(prev) && is_cjk(next) && !it.marks.contains(&c) {
            row.half_word.push(shown.clone());
        }
        if JUDGE_FUNC.contains(prev) {
            row.after_func.push(shown.clone());
        }
        if JUDGE_PARTICLE.contains(next) {
            row.before_particle.push(shown.clone());
        }
        let wordy = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | '@' | ':' | '/');
        let in_token = {
            let (mut a, mut b) = (c, c);
            while a > 0 && !it.raw[a - 1].is_whitespace() {
                a -= 1;
            }
            while b < it.raw.len() && !it.raw[b].is_whitespace() {
                b += 1;
            }
            let t: String = it.raw[a..b].iter().collect();
            t.contains("://") || t.contains('@')
        };
        if (wordy(prev) && wordy(next) && !(prev == ',' && !next.is_ascii_digit())) || (prev.is_ascii_digit() && is_cjk(next)) || in_token {
            row.inside_ascii.push(shown);
        }
    }
    row
}

fn mean(v: &[usize]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<usize>() as f64 / v.len() as f64
    }
}

fn show(label: &str, cfg: &EarlyConfig, which: Option<u8>) -> Row {
    let r = run(cfg, which);
    eprintln!(
        "{label:<22} n_min={:<2} n_max={:<2} plain={:<5} | n={:<3} cut={:<3} ({:>3.0}%) no_cut_short={:<2} first_chars mean={:.1} min={} saved_chars mean={:.1} | ugly={} (half_word={} after_func={} before_particle={} inside_ascii={})",
        cfg.n_min,
        cfg.n_max,
        cfg.plain_fallback,
        r.n,
        r.cut,
        100.0 * r.cut as f64 / r.n.max(1) as f64,
        r.too_short,
        mean(&r.first_chars),
        r.first_chars.iter().min().copied().unwrap_or(0),
        mean(&r.saved_chars),
        r.ugly(),
        r.half_word.len(),
        r.after_func.len(),
        r.before_particle.len(),
        r.inside_ascii.len(),
    );
    r
}

#[test]
fn the_corpus_is_big_enough_and_split_into_tune_and_heldout() {
    let c = corpus();
    let n = |f: &dyn Fn(&Item) -> bool| c.iter().filter(|i| f(i)).count();
    assert!(n(&|i| !i.adversarial) >= 40 && n(&|i| i.heldout) >= 20 && n(&|i| i.adversarial) >= 10, "{}", c.len());
}

/// 调参的格子：只打印，不断言（断言在下一个测试，钉的是最终选的那一组）。
#[test]
fn grid_report() {
    for (lo, hi) in [(6, 10), (8, 12), (8, 14), (10, 14), (10, 16), (12, 18)] {
        for plain in [false, true] {
            show("all", &EarlyConfig { n_min: lo, n_max: hi, plain_fallback: plain }, None);
        }
    }
    let r = run(&EarlyConfig { plain_fallback: true, ..EarlyConfig::default() }, None);
    for u in &r.half_word {
        eprintln!("  plain 回退的丑切点：{u}");
    }
    for plain in [false, true] {
        let r = show("adversarial", &EarlyConfig { plain_fallback: plain, ..EarlyConfig::default() }, Some(2));
        for u in r.half_word.iter().chain(&r.after_func).chain(&r.before_particle).chain(&r.inside_ascii) {
            eprintln!("  丑切点：{u}");
        }
    }
}

#[test]
fn the_default_config_has_no_ugly_cut_on_either_set_and_cuts_a_useful_share() {
    let cfg = EarlyConfig::default();
    for (label, which) in [("tune", Some(0)), ("heldout", Some(1))] {
        let r = show(label, &cfg, which);
        for u in r.half_word.iter().chain(&r.after_func).chain(&r.before_particle).chain(&r.inside_ascii) {
            eprintln!("  丑切点：{u}");
        }
        assert_eq!(r.ugly(), 0, "{label}: {r:?}");
        assert!(r.first_chars.iter().all(|&n| n >= cfg.n_min as usize - 2), "{label}: 首段太短 {:?}", r.first_chars);
        assert!(r.cut * 100 / r.n >= 25, "{label}: 切得出的太少 {}/{}", r.cut, r.n);
    }
}

#[test]
fn every_cut_piece_joins_back_to_the_sentence_and_the_rest_is_not_empty_after_cleaning() {
    let cfg = EarlyConfig::default();
    for it in corpus().iter().filter(|i| !i.adversarial) {
        let limit = pending_len(&it.raw);
        for p in 1..=limit {
            let prefix: String = it.raw[..p].iter().collect();
            if let Some(b) = first_cut(&prefix, &cfg) {
                let whole: String = it.raw[..limit].iter().collect();
                let (h, r) = whole.split_at(b);
                assert_eq!(format!("{h}{r}"), whole);
                assert!(r.chars().next().is_some_and(char::is_alphanumeric), "后半截要以字母数字开头：{h}｜{r}");
                break;
            }
        }
    }
}
