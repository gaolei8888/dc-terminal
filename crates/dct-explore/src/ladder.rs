//! 一道题怎么逐级往上问：Jeff（够把握才算）→ 大模型（同一画面最多 2 次）→ 用户。
//! 这里只决定「谁来答」；危险与否由 gate.rs 管，绝不看模型的把握。
use crate::options::{Act, Opt};

/// 谁答的。记进轨迹的 `by` 字段，所以要能来回转 JSON。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    Jeff,
    Model,
    User,
}

pub struct JeffAnswer {
    pub key: String,
    pub probabilities: Vec<(String, f64)>,
    pub answer_confidence: f64,
}
/// None = 没配 / 连不上 / 忙。
pub trait Jeff {
    fn ask(&self, state: &str, q: &crate::JevChoice) -> Option<JeffAnswer>;
}

pub struct ModelPick {
    pub key: String,
    pub name: Option<String>,
}
/// None = 卡住 / 读不懂 / 没配。
pub trait Model {
    fn pick(&self, state: &str, all: &[Opt], picture_png: Option<&str>) -> Option<ModelPick>;
}

pub struct UserPick {
    pub key: String,
    pub name: Option<String>,
}
/// None = 用户说停。
pub trait User {
    fn pick(
        &mut self,
        state: &str,
        all: &[Opt],
        picture_png: Option<&str>,
        why: &str,
    ) -> Option<UserPick>;
}

pub struct NoModel;
impl Model for NoModel {
    fn pick(&self, _: &str, _: &[Opt], _: Option<&str>) -> Option<ModelPick> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub key: String,
    pub name: Option<String>,
    pub by: Who,
    /// Jeff 当时的答案（有的话），用来存训练题。
    pub jeff: Option<(String, f64)>,
}

pub struct Ladder<'a> {
    pub jeff: &'a dyn Jeff,
    pub model: &'a dyn Model,
    pub user: &'a mut dyn User,
    pub threshold: f64,
}

/// 同一画面（scene_hash）上的失败次数，由调用方保管：执行后画面没变 / 绕回 → 调用方记一笔。
#[derive(Default)]
pub struct Tries {
    pub jeff_failed: bool,
    pub model_tries: u32,
}

/// 大模型在同一画面最多试这么多次。
const MODEL_MAX_TRIES: u32 = 2;
/// 「卡住了」选项的 key（options::question 里固定）。
const STUCK_KEY: &str = "stuck";

fn find<'o>(opts: &'o [Opt], key: &str) -> Option<&'o Opt> {
    opts.iter().find(|o| o.key == key)
}

/// 选项能不能直接用：不是「卡住」，没字图标要带名字（Jeff 起不了名字，大模型要给）。
fn usable(opt: &Opt, name: Option<&str>) -> bool {
    if opt.key == STUCK_KEY {
        return false;
    }
    match &opt.act {
        Act::TapIcon { name: preset, .. } => {
            name.is_some_and(|n| !n.trim().is_empty())
                || preset.as_deref().is_some_and(|n| !n.trim().is_empty())
        }
        _ => true,
    }
}

impl<'a> Ladder<'a> {
    /// 返回 None = 用户说停。
    pub fn decide(
        &mut self,
        state: &str,
        short: &[Opt],
        all: &[Opt],
        picture: Option<&str>,
        tries: &Tries,
    ) -> Option<Decision> {
        let mut why: Vec<String> = Vec::new();

        // 第 1 级：Jeff
        let mut jeff_seen: Option<(String, f64)> = None;
        if tries.jeff_failed {
            why.push("Jeff 选的那一下没起作用".into());
        } else {
            let q = crate::options::question(short);
            match self.jeff.ask(state, &q) {
                None => why.push("Jeff 没有回答".into()),
                Some(a) => {
                    jeff_seen = Some((a.key.clone(), a.answer_confidence));
                    let opt = find(short, &a.key).or_else(|| find(all, &a.key));
                    if a.answer_confidence < self.threshold {
                        why.push(format!("Jeff 没有把握（{:.2}）", a.answer_confidence));
                    } else if let Some(o) = opt.filter(|o| usable(o, None)) {
                        return Some(Decision {
                            key: o.key.clone(),
                            name: None,
                            by: Who::Jeff,
                            jeff: jeff_seen,
                        });
                    } else if opt.is_some_and(|o| matches!(o.act, Act::TapIcon { .. })) {
                        why.push("这个图标没有名字".into());
                    } else {
                        why.push("Jeff 选了「卡住了」或看不懂的选项".into());
                    }
                }
            }
        }

        // 第 2 级：大模型
        if tries.model_tries >= MODEL_MAX_TRIES {
            why.push(format!("大模型已经试了 {} 次", MODEL_MAX_TRIES));
        } else {
            match self.model.pick(state, all, picture) {
                None => why.push("大模型说卡住了或没读懂".into()),
                Some(p) => match find(all, &p.key).filter(|o| usable(o, p.name.as_deref())) {
                    Some(o) => {
                        return Some(Decision {
                            key: o.key.clone(),
                            name: p.name,
                            by: Who::Model,
                            jeff: jeff_seen,
                        })
                    }
                    None => why.push("大模型说卡住了或没读懂".into()),
                },
            }
        }

        // 第 3 级：用户
        let p = self.user.pick(state, all, picture, &why.join("，"))?;
        Some(Decision {
            key: p.key,
            name: p.name,
            by: Who::User,
            jeff: jeff_seen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Act, Opt};
    use std::cell::RefCell;

    struct J(Option<(&'static str, f64)>);
    impl Jeff for J {
        fn ask(&self, _: &str, _: &crate::JevChoice) -> Option<JeffAnswer> {
            self.0.map(|(k, c)| JeffAnswer {
                key: k.into(),
                probabilities: vec![(k.into(), c)],
                answer_confidence: c,
            })
        }
    }
    struct M(Option<(&'static str, Option<&'static str>)>, RefCell<u32>);
    impl Model for M {
        fn pick(&self, _: &str, _: &[Opt], _: Option<&str>) -> Option<ModelPick> {
            *self.1.borrow_mut() += 1;
            self.0.map(|(k, n)| ModelPick {
                key: k.into(),
                name: n.map(String::from),
            })
        }
    }
    struct U(Option<(&'static str, Option<&'static str>)>, Vec<String>);
    impl User for U {
        fn pick(&mut self, _: &str, _: &[Opt], _: Option<&str>, why: &str) -> Option<UserPick> {
            self.1.push(why.into());
            self.0.map(|(k, n)| UserPick {
                key: k.into(),
                name: n.map(String::from),
            })
        }
    }

    fn opts() -> Vec<Opt> {
        vec![
            Opt {
                key: "e3".into(),
                text: "按钮 Post".into(),
                act: Act::Stuck,
                publish: true,
            },
            Opt {
                key: "e5".into(),
                text: "没有字的图标".into(),
                act: Act::TapIcon {
                    el: crate::hands::El {
                        id: "e5".into(),
                        text: "".into(),
                        role: "image".into(),
                        frac: crate::hands::Frac {
                            x: 0.0,
                            y: 0.0,
                            w: 0.0,
                            h: 0.0,
                        },
                        source: "detector".into(),
                    },
                    name: None,
                },
                publish: false,
            },
            Opt {
                key: "stuck".into(),
                text: "卡住了".into(),
                act: Act::Stuck,
                publish: false,
            },
        ]
    }

    #[test]
    fn a_confident_jeff_answers_alone() {
        let (j, m, mut u) = (
            J(Some(("e3", 0.93))),
            M(Some(("e5", Some("add media"))), RefCell::new(0)),
            U(None, vec![]),
        );
        let d = Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8,
        }
        .decide("s", &opts(), &opts(), None, &Tries::default())
        .unwrap();
        assert_eq!((d.key.as_str(), d.by), ("e3", Who::Jeff));
        assert_eq!(*m.1.borrow(), 0, "Jeff 有把握就不叫大模型");
    }

    #[test]
    fn low_confidence_goes_to_the_model_and_keeps_jeffs_answer_for_training() {
        let (j, m, mut u) = (
            J(Some(("stuck", 0.42))),
            M(Some(("e3", None)), RefCell::new(0)),
            U(None, vec![]),
        );
        let d = Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8,
        }
        .decide("s", &opts(), &opts(), None, &Tries::default())
        .unwrap();
        assert_eq!((d.key.as_str(), d.by), ("e3", Who::Model));
        assert_eq!(d.jeff, Some(("stuck".into(), 0.42)));
    }

    #[test]
    fn jeff_cannot_name_an_icon_so_a_confident_icon_pick_still_goes_up() {
        let (j, m, mut u) = (
            J(Some(("e5", 0.95))),
            M(Some(("e5", Some("add media"))), RefCell::new(0)),
            U(None, vec![]),
        );
        let d = Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8,
        }
        .decide("s", &opts(), &opts(), None, &Tries::default())
        .unwrap();
        assert_eq!((d.by, d.name.as_deref()), (Who::Model, Some("add media")));
    }

    #[test]
    fn after_a_failed_jeff_pick_jeff_is_skipped_on_the_same_screen() {
        let (j, m, mut u) = (
            J(Some(("e3", 0.99))),
            M(Some(("e3", None)), RefCell::new(0)),
            U(None, vec![]),
        );
        let tries = Tries {
            jeff_failed: true,
            model_tries: 0,
        };
        let d = Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8,
        }
        .decide("s", &opts(), &opts(), None, &tries)
        .unwrap();
        assert_eq!(d.by, Who::Model);
    }

    #[test]
    fn model_stuck_malformed_or_two_tries_goes_to_the_user() {
        for (model, tries) in [
            (None, 0),
            (Some(("stuck", None)), 0),
            (Some(("e3", None)), 2),
            (Some(("nope", None)), 0),
            (Some(("e5", None)), 0),
        ] {
            let (j, m, mut u) = (
                J(None),
                M(model, RefCell::new(0)),
                U(Some(("e3", None)), vec![]),
            );
            let d = Ladder {
                jeff: &j,
                model: &m,
                user: &mut u,
                threshold: 0.8,
            }
            .decide(
                "s",
                &opts(),
                &opts(),
                None,
                &Tries {
                    jeff_failed: false,
                    model_tries: tries,
                },
            )
            .unwrap();
            assert_eq!(d.by, Who::User, "{model:?} {tries}");
            assert!(!u.1[0].is_empty(), "要告诉用户为什么问他");
        }
    }

    #[test]
    fn the_user_saying_stop_ends_it() {
        let (j, m, mut u) = (J(None), M(None, RefCell::new(0)), U(None, vec![]));
        assert!(Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8
        }
        .decide("s", &opts(), &opts(), None, &Tries::default())
        .is_none());
    }
}
