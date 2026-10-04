//! 不看数字的意思，只看数字怎么变，认出哪个是「目标」。
//! 步数每成功一步刚好降 1；分数、连击只增不减；目标是降过至少一次、又不是上面两种的那个。
//! 从没变过的数字永远不认。恰好剩一个才认；多于一个或没有就不认（宁可不判停滞，也不乱判）。

pub const MIN_SAMPLES: usize = 4;

#[derive(Default)]
pub struct GoalFinder {
    samples: usize,
    all_minus_one: Vec<bool>,
    ever_increased: Vec<bool>,
    ever_decreased: Vec<bool>,
    /// 第一个样本的 `before`：认出目标后，用它当「开局值」。
    first_before: Vec<u64>,
    found: Option<usize>,
}

impl GoalFinder {
    pub fn new() -> GoalFinder {
        GoalFinder::default()
    }

    /// 只在**成功走了一步**之后调用。前后个数不同的步不进样本（OCR 漏读、弹窗飘字）。
    pub fn observe(&mut self, before: &[u64], after: &[u64]) {
        if self.found.is_some() || before.len() != after.len() || before.is_empty() {
            return;
        }
        if self.samples == 0 {
            self.all_minus_one = vec![true; before.len()];
            self.ever_increased = vec![false; before.len()];
            self.ever_decreased = vec![false; before.len()];
            self.first_before = before.to_vec();
        }
        if self.all_minus_one.len() != before.len() {
            return; // 列表个数和已有样本不一致：这一步不进样本
        }
        self.samples += 1;
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            if *a != b.saturating_sub(1) || *b == 0 {
                self.all_minus_one[i] = false;
            }
            if a > b {
                self.ever_increased[i] = true;
            }
            if a < b {
                self.ever_decreased[i] = true;
            }
        }
        if self.samples >= MIN_SAMPLES {
            let left: Vec<usize> = (0..self.all_minus_one.len()).filter(|&i| !self.all_minus_one[i] && !self.ever_increased[i] && self.ever_decreased[i]).collect();
            if left.len() == 1 {
                self.found = Some(left[0]);
            }
        }
    }

    pub fn index(&self) -> Option<usize> {
        self.found
    }

    /// 目标认出之后，步数那一列的下标：每个样本都刚好 -1 的唯一一列（恰好一个才认）。
    pub fn step_index(&self) -> Option<usize> {
        self.found?;
        if self.samples < MIN_SAMPLES {
            return None;
        }
        let mut it = self.all_minus_one.iter().enumerate().filter(|(_, &m)| m).map(|(i, _)| i);
        let first = it.next()?;
        if it.next().is_some() {
            return None;
        }
        Some(first)
    }

    /// 目标认出时，目标那一列的开局值（第一个样本的 `before`）。
    pub fn goal_start(&self) -> Option<u64> {
        self.first_before.get(self.found?).copied()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn feed(f: &mut GoalFinder, steps: &[(&[u64], &[u64])]) {
        for (b, a) in steps {
            f.observe(b, a);
        }
    }

    #[test]
    fn the_step_counter_and_a_rising_score_are_excluded_leaving_the_goal() {
        // [步数, 目标, 分数]：步数每步 -1，目标不规律降，分数涨
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn a_number_that_never_moves_is_never_picked_as_the_goal() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[20, 50, 0], &[19, 50, 100]),
            (&[19, 50, 100], &[18, 50, 150]),
            (&[18, 50, 150], &[17, 50, 400]),
            (&[17, 50, 400], &[16, 50, 410]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn a_goal_that_dropped_once_is_found() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[20, 50, 0], &[19, 50, 100]),
            (&[19, 50, 100], &[18, 49, 150]),
            (&[18, 49, 150], &[17, 49, 400]),
            (&[17, 49, 400], &[16, 49, 410]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn two_irregular_numbers_are_ambiguous() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 30], &[37, 110, 25]),
            (&[37, 110, 25], &[36, 100, 20]),
            (&[36, 100, 20], &[35, 100, 15]),
            (&[35, 100, 15], &[34, 80, 5]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn fewer_than_four_samples_decide_nothing() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36, 100]),
            (&[36, 100], &[35, 90]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn steps_with_different_list_lengths_are_not_samples() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36]),            // 漏读一个：不进样本
            (&[36, 100], &[35, 100, 9]),    // 多出一个：不进样本
            (&[35, 100], &[34, 90]),
            (&[34, 90], &[33, 80]),
        ]);
        assert_eq!(f.index(), None, "只有 3 个有效样本");
        feed(&mut f, &[(&[33, 80], &[32, 70])]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn step_index_is_the_one_column_that_dropped_by_one_every_time() {
        let mut f = GoalFinder::new();
        assert_eq!(f.step_index(), None);
        feed(&mut f, &[
            (&[122, 38, 0], &[110, 37, 120]),
            (&[110, 37, 120], &[100, 36, 300]),
            (&[100, 36, 300], &[100, 35, 340]),
        ]);
        assert_eq!(f.step_index(), None, "目标还没认出");
        feed(&mut f, &[(&[100, 35, 340], &[80, 34, 600])]);
        assert_eq!(f.index(), Some(0));
        assert_eq!(f.step_index(), Some(1));
    }

    #[test]
    fn step_index_is_none_when_two_columns_drop_by_one_every_time() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 38], &[37, 110, 37]),
            (&[37, 110, 37], &[36, 100, 36]),
            (&[36, 100, 36], &[35, 100, 35]),
            (&[35, 100, 35], &[34, 80, 34]),
        ]);
        assert_eq!(f.index(), Some(1));
        assert_eq!(f.step_index(), None);
    }

    #[test]
    fn goal_start_is_the_goal_number_of_the_first_sample() {
        let mut f = GoalFinder::new();
        assert_eq!(f.goal_start(), None);
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
        ]);
        assert_eq!(f.goal_start(), None, "还没认出");
        feed(&mut f, &[(&[35, 100, 340], &[34, 80, 600])]);
        assert_eq!(f.goal_start(), Some(122));
    }

    #[test]
    fn once_found_the_goal_stays_found_for_the_run() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
        // 之后目标数字涨了一次（重置）也不改
        feed(&mut f, &[(&[34, 80, 600], &[33, 90, 650])]);
        assert_eq!(f.index(), Some(1));
    }
}
