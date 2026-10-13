"""Brain-vs-hand danger report: every tappable label in the dev scenario pools (base, wide, wide2) and the exam scenario pools, where
dct-brain's verdict differs from the hand flag, with the class from danger_allowlist.json.  READ-ONLY: nothing under jeff-exam is written.

    python3 -I scripts/jeff-exam/diff_danger.py [--out report.md]

brain verdict = tier_for_label in Content/Critical/Money for BOTH the en and the zh text (en != zh = "brain inconsistent").
hand flag = dev: the hand rule build_exam.rule_danger; exam: the `d` flag in the frozen scenario files (the only flag the exam has).
Classes (danger_allowlist.json): a = brain misses a destructive/outward action; b = brain false positive or a different question
(save/share/sign-in...); c = our flag is wrong. "n items" = how many items of dev-v3 / exam-v2 offer the label as an option.
"""
import argparse
import collections
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_exam import ALLOWLIST, BRAIN_DANGER, DEV_PARTS, REPO, brain_tiers, load_scen, rule_danger  # noqa: E402
from exam_lib import load_items  # noqa: E402

FIX = os.path.join(REPO, "tests", "fixtures")
# probes for the three false positives dct-brain's owners already know (and one gap), run through the same binary
PROBES = ["不删除", "Don't delete", "Delete", "~~Delete~~", "清空筛选", "Clear filters", "Save for later", "Cancel my booking", "Cancel", "取消"]


def pool(dirs):
    rows = {}
    for d in dirs:
        for x in sum(load_scen(d), []):
            for _, en, zh, fl in x["labels"]:
                if any(c in fl for c in "bt"):
                    rows.setdefault((en, zh), set()).add("d" in fl)
    return rows


def offered(path):
    n = collections.Counter()
    for it in load_items(path):
        if it["lang"] == "en":
            for e in it["elements"]:
                if e["role"] in "bt":
                    n[e["text"]] += 1
    return n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out")
    a = ap.parse_args()
    groups = json.load(open(ALLOWLIST, encoding="utf-8"))["groups"]
    cls = {("dev", en): g for g in groups for en in g["dev_labels"]}
    cls.update({("exam", en): g for g in groups for en in g["exam_labels"]})
    sets = {
        "dev": (pool([os.path.join(FIX, "jeff-dev", c["dir"]) for c in DEV_PARTS]), offered(os.path.join(FIX, "jeff-dev", "dev-v3.jsonl"))),
        "exam": (pool([os.path.join(FIX, "jeff-exam", "scenarios")]), offered(os.path.join(FIX, "jeff-exam", "exam-v2.jsonl"))),
    }
    texts = sorted({x for rows, _ in sets.values() for p in rows for x in p} | set(PROBES))
    tier = dict(zip(texts, brain_tiers(texts)))
    isd = lambda t: t in BRAIN_DANGER  # noqa: E731
    L = ["# dct-brain vs hand danger flag", "",
         "brain verdict = `tier_for_label` in content/critical/money in BOTH languages; hand flag = dev: build_exam.rule_danger, exam: the `d` flag in the "
         "frozen scenario files. Pools are the scenario label pools (a superset of the labels that ended up in items; `n items` counts the items that "
         "offer the label, en side). Exam files are read only.", ""]
    summary = {}
    detail = collections.defaultdict(list)
    for sn, (rows, off) in sets.items():
        c = collections.Counter()
        for (en, zh), flags in sorted(rows.items()):
            hand = rule_danger(en, "b") if sn == "dev" else any(flags)
            be, bz = isd(tier[en]), isd(tier[zh])
            c["labels"] += 1
            if be == bz == hand:
                c["agree"] += 1
                continue
            g = cls.get((sn, en))
            k = g["class"] if g else "UNCLASSIFIED"
            c["differ"] += 1
            c[k] += 1
            c["en/zh inconsistent"] += be != bz
            detail[(sn, k)].append((en, zh, hand, tier[en], tier[zh], off.get(en, 0), g["id"] if g else "-"))
        summary[sn] = c
    L += ["## Counts (distinct en/zh label pairs)", "", "| set | labels | agree | differ | a vocab gap | b brain FP / scope | c our flag wrong | unclassified | of differ: brain en != zh |", "|---|---|---|---|---|---|---|---|---|"]
    for sn, c in summary.items():
        L.append(f"| {sn} | {c['labels']} | {c['agree']} | {c['differ']} | {c['a']} | {c['b']} | {c['c']} | {c['UNCLASSIFIED']} | {c['en/zh inconsistent']} |")
    L += ["", "Groups (class, sub-kind, flag the dev build uses, how many dev / exam labels):", ""]
    for g in groups:
        L.append(f"- `{g['id']}` class {g['class']} ({g['sub']}), flag {g['flag']}, {len(g['dev_labels'])} dev / {len(g['exam_labels'])} exam: {g['reason']}")
    L += ["", "## Known false positives, probed directly through the same binary", "", "| text | tier |", "|---|---|"] + [f"| {p} | {tier[p]} |" for p in PROBES]
    L += ["", "(The three known false positives are 不删除, a struck-through label and 清空筛选; none of them is in any existing label pool, so they do not "
          "appear below. Brain also reads a leading Cancel/取消 as a refusal, which is the gap behind class a-cancel-is-refusal.)", ""]
    for (sn, k), rows in sorted(detail.items()):
        L += [f"## {sn}, class {k} ({len(rows)})", "", "| en | zh | hand | brain en | brain zh | n items | group |", "|---|---|---|---|---|---|---|"]
        L += [f"| {en} | {zh} | {'danger' if h else 'no'} | {te} | {tz} | {n} | {g} |" for en, zh, h, te, tz, n, g in rows]
        L.append("")
    md = "\n".join(L) + "\n"
    if a.out:
        open(a.out, "w", encoding="utf-8").write(md)
    print(md if not a.out else "\n".join(L[:12 + len(sets)]))
    if any(c["UNCLASSIFIED"] for c in summary.values()):
        sys.exit("unclassified differences: add them to danger_allowlist.json")


if __name__ == "__main__":
    main()
