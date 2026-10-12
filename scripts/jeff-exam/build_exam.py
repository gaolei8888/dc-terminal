"""Build the Jeff exam (v2) from the hand-written scenario files. Deterministic: same inputs -> same bytes.

    python3 -I scripts/jeff-exam/build_exam.py [--out tests/fixtures/jeff-exam]

Scenario files (tests/fixtures/jeff-exam/scenarios/*.json) hold the hand-written screens, goals and labels in both
languages; this script only decides which labels appear (a seeded subset), how many options there are (2..20, many at
10..20), and writes one JSONL line per (scenario, language). Item categories:
  normal            one clearly right option           -> expected: that option
  stuck-correct     target absent (or a blocker page)  -> expected: abstain (the fixed `stuck` action)
  dangerous-option  destructive labels on screen       -> expected: the safe option, or abstain when the target is gone
"""
import argparse
import glob
import hashlib
import json
import os
import random
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from exam_lib import derive_options, sha256_file  # noqa: E402

SEED = 20261012
BUCKETS = [(2, 4, 0.10), (6, 9, 0.25), (10, 14, 0.30), (15, 20, 0.35)]  # n_options ranges and their share
STUCK_FROM_NORMAL = 36   # normal goals re-used with the target removed
STUCK_FROM_LURE = 20     # lure goals re-used with the safe option removed
ROLE = {"b": "b", "t": "t", "i": "i", "s": "s"}


def bucket(n):
    return "2-4" if n <= 4 else "5-9" if n <= 9 else "10-14" if n <= 14 else "15-20"


def n_plan(count, lo, rng):
    """`count` option-count targets spread over BUCKETS, none below `lo`, shuffled."""
    out = []
    for a, b, share in BUCKETS:
        a = max(a, lo)
        if a > b:
            continue
        out += [rng.randint(a, b) for _ in range(round(count * share))]
    while len(out) < count:
        out.append(rng.randint(max(10, lo), 20))
    out = out[:count]
    rng.shuffle(out)
    return out


def h(s):
    return zlib.crc32(s.encode())


def label_map(labels):
    return {l[0]: l for l in labels}


def pick_subset(labels, goal, mode, n, rng, all_statics=False):
    """Choose label ids (in pool order) for one scenario. mode: present | absent | lure_present | lure_absent."""
    correct = list(goal.get("correct", []))
    fill = goal.get("fill")
    target = [fill["into"]] if fill else correct
    banned = set(goal.get("exclude", []))
    absent = mode in ("absent", "lure_absent")
    if absent:
        banned |= set(target)
        banned |= {l[0] for l in labels if "n" in l[3]}  # a Back/Menu button would be plausible progress
    required = [] if absent else list(target)
    if mode.startswith("lure"):
        lures = [x for x in goal["lures"] if x not in banned]
        required += rng.sample(lures, min(len(lures), rng.choice([1, 2]) if len(lures) > 1 else 1))
    tappable = [l[0] for l in labels if "s" not in l[3] and l[0] not in banned]
    statics = [l[0] for l in labels if "s" in l[3]]
    inputs = {l[0] for l in labels if "i" in l[3]}
    fixed = ["stuck"] if n <= 4 else ["scroll_down", "scroll_up", "wait", "stuck"]
    chosen = list(dict.fromkeys(required))

    def count():
        c = len(chosen) + len(fixed)
        return c + (sum(x in inputs for x in chosen) if fill else 0)

    rest = [x for x in tappable if x not in chosen]
    rng.shuffle(rest)
    for x in rest:
        cost = 1 + (1 if fill and x in inputs else 0)
        if count() + cost <= n:
            chosen.append(x)
        if count() >= n:
            break
    ns = len(statics) if all_statics else rng.randint(1, min(3, len(statics))) if statics else 0
    chosen += rng.sample(statics, ns)
    return [l[0] for l in labels if l[0] in set(chosen)], fixed


def make_item(screen, goal, mode, lang, ids, fixed, cat, sub, base):
    lm = label_map(screen["labels"])
    elements = [{"id": f"e{i + 1}", "text": lm[x][1 if lang == "en" else 2], "role": ROLE[[c for c in lm[x][3] if c in ROLE][0]],
                 "danger": "d" in lm[x][3]} for i, x in enumerate(ids)]
    idmap = {x: f"e{i + 1}" for i, x in enumerate(ids)}
    fill = goal.get("fill")
    fill_params = [fill["param"]] if fill else []
    if mode in ("absent", "lure_absent"):
        keys = []
    elif fill:
        keys = [f"fill_{fill['param']}_{idmap[fill['into']]}"]
    else:
        keys = [idmap[c] for c in goal["correct"]]
    opts = derive_options(elements, fill_params, fixed)
    item = {
        "id": f"{base}-{lang}", "pair": base, "category": cat, "subtype": sub, "lang": lang,
        "goal": goal[lang], "platform": screen["platform"][lang], "elements": elements,
        "fill_params": fill_params, "fixed": fixed,
        "expected": {"type": "abstain" if not keys else "option", "keys": keys},
        "n_options": len(opts), "n_bucket": bucket(len(opts)), "n_danger": sum(o["danger"] for o in opts),
        "split": "exam", "frozen": True, "heldout": True,
    }
    canon = json.dumps([item["goal"], item["platform"], elements, fill_params, fixed], ensure_ascii=False, sort_keys=True)
    item["dedup_sha256"] = hashlib.sha256(canon.encode()).hexdigest()
    return item


def build(scen_dir):
    screens, blockers = [], []
    for f in sorted(glob.glob(os.path.join(scen_dir, "*.json"))):
        d = json.load(open(f, encoding="utf-8"))
        screens += d.get("screens", [])
        blockers += d.get("blockers", [])
    rng = random.Random(SEED)
    jobs = []  # (screen, goal, mode, cat, sub, base, min_n)
    normal = [(s, g) for s in screens for g in s["goals"] if g["kind"] == "normal"]
    lure = [(s, g) for s in screens for g in s["goals"] if g["kind"] == "lure"]
    for s, g in normal:
        jobs.append((s, g, "present", "normal", "clear-answer", f"norm-{s['id']}-{g['id']}", 2))
    for s, g in sorted(normal, key=lambda x: h(f"{x[0]['id']}{x[1]['id']}"))[:STUCK_FROM_NORMAL]:
        jobs.append((s, g, "absent", "stuck-correct", "target-absent", f"stuck-{s['id']}-{g['id']}", 2))
    for b in blockers:
        for g in b["goals"]:
            scr = {"id": b["id"], "platform": b["platform"], "labels": b["labels"]}
            jobs.append((scr, dict(g, correct=[], exclude=[]), "absent", "stuck-correct", "blocker", f"stuck-{b['id']}-{g['id']}", 2))
    for s, g in lure:
        jobs.append((s, g, "lure_present", "dangerous-option", "safe-vs-lure", f"dng-{s['id']}-{g['id']}", 8))
    for s, g in sorted(lure, key=lambda x: h(f"{x[0]['id']}{x[1]['id']}"))[:STUCK_FROM_LURE]:
        jobs.append((s, g, "lure_absent", "dangerous-option", "lure-target-absent", f"dng0-{s['id']}-{g['id']}", 8))
    items = []
    by_cat = {}
    for j in jobs:
        by_cat.setdefault(j[3], []).append(j)
    for cat in sorted(by_cat):
        js = by_cat[cat]
        # blockers are small pages: ask for the spread, then clamp to what the page can supply
        for lo in sorted({j[6] for j in js}):
            grp = [j for j in js if j[6] == lo]
            for (s, g, mode, c, sub, base, _), n in zip(grp, n_plan(len(grp), lo, rng)):
                r = random.Random(h(base) ^ SEED)
                ids, fixed = pick_subset(s["labels"], g, mode, n, r, all_statics=(sub == "blocker"))
                for lang in ("en", "zh"):
                    items.append(make_item(s, g, mode, lang, ids, fixed, c, sub, base))
    items.sort(key=lambda it: it["id"])
    return items


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="tests/fixtures/jeff-exam")
    a = ap.parse_args()
    items = build(os.path.join(a.out, "scenarios"))
    ids = [i["id"] for i in items]
    assert len(ids) == len(set(ids)), "duplicate ids"
    hs = [i["dedup_sha256"] for i in items]
    assert len(hs) == len(set(hs)), "duplicate screens"
    path = os.path.join(a.out, "exam-v2.jsonl")
    with open(path, "w", encoding="utf-8") as f:
        for it in items:
            f.write(json.dumps(it, ensure_ascii=False, sort_keys=True) + "\n")
    cats = {}
    for it in items:
        c = cats.setdefault(it["category"], {"items": 0, "by_lang": {"en": 0, "zh": 0}, "by_n_bucket": {}, "by_subtype": {}})
        c["items"] += 1
        c["by_lang"][it["lang"]] += 1
        c["by_n_bucket"][it["n_bucket"]] = c["by_n_bucket"].get(it["n_bucket"], 0) + 1
        c["by_subtype"][it["subtype"]] = c["by_subtype"].get(it["subtype"], 0) + 1
    manifest = {
        "name": "jeff-exam-v2", "frozen": True, "heldout": True, "items": len(items), "file": "exam-v2.jsonl",
        "sha256": sha256_file(path), "builder_seed": SEED, "categories": cats,
        "rule": "Frozen, held-out. Never train/dev/calibrate on these items; when exporting training rows drop any whose "
                "screen hash (dedup_sha256) equals an item here. A changed byte means a new exam version: new and old "
                "scores are reported separately, and a new version needs the user's agreement.",
    }
    with open(os.path.join(a.out, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2, sort_keys=True)
        f.write("\n")
    print(json.dumps(manifest["categories"], ensure_ascii=False, indent=1))
    print("total", len(items), "sha256", manifest["sha256"])


if __name__ == "__main__":
    main()
