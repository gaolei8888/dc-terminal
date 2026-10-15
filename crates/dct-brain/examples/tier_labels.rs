//! Prints `tier_for_label` for each line on stdin (one label per line) as `<tier>\t<label>`.
//! Used by scripts/jeff-exam/build_exam.py so the dev danger flags come from the real classifier.
//! No new API: it only calls the existing public `tier::tier_for_label`.
use std::io::BufRead;

fn main() {
    for line in std::io::stdin().lock().lines() {
        let label = line.expect("stdin");
        println!(
            "{}\t{}",
            dct_brain::tier::tier_for_label(&label).name(),
            label
        );
    }
}
