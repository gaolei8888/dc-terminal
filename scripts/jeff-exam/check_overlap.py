"""Dev/exam leak check: no dev item may share a screen or a goal with the frozen exam, and no wide-dev item with a base-dev item.

    python3 -I scripts/jeff-exam/check_overlap.py [--max 0.5]

Compares, within the same language, (a) screen text = the page's element texts sorted and joined, (b) goal text. Reports exact
matches (incl. dedup_sha256) and the maximum character 3-gram Jaccard similarity (works for en and zh), with the closest pair.
Exit 1 if anything matches exactly or the maximum similarity exceeds --max.
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from exam_lib import load_items  # noqa: E402

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tests", "fixtures")


def grams(t):
    t = re.sub(r"\s+", " ", t.lower()).strip()
    return {t[i:i + 3] for i in range(max(1, len(t) - 2))}


def jac(a, b):
    return len(a & b) / len(a | b) if a | b else 0.0


def screen_text(it):
    return "\n".join(sorted(e["text"] for e in it["elements"]))


def sims(left, right, text):
    """max 3-gram Jaccard (same language) of `text` over every left x right pair, and the pair."""
    rg = [(r["id"], r["lang"], grams(text(r))) for r in right]
    best = (0.0, None, None)
    for d in left:
        g = grams(text(d))
        for rid, rl, rgm in rg:
            if rl == d["lang"] and rid != d["id"]:
                s = jac(g, rgm)
                if s > best[0]:
                    best = (s, d["id"], rid)
    return best


def check(name, left, right, mx):
    bad = False
    print(f"{name}: {len(left)} items vs {len(right)} items; character 3-gram Jaccard, same language only")
    hashes = {e["dedup_sha256"] for e in right}
    h = [d["id"] for d in left if d["dedup_sha256"] in hashes]
    print(f"  dedup_sha256 collisions: {len(h)}")
    bad |= bool(h)
    for what, text in (("screen", screen_text), ("goal", lambda i: i["goal"])):
        right_t = {text(e) for e in right}
        exact = [d["id"] for d in left if text(d) in right_t]
        s, a, b = sims(left, right, text)
        print(f"  {what}: exact matches {len(exact)}; max 3-gram Jaccard {s:.3f} ({a} vs {b})")
        bad |= bool(exact) or s > mx
    return bad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--max", type=float, default=0.5)
    a = ap.parse_args()
    exam = load_items(os.path.join(ROOT, "jeff-exam", "exam-v2.jsonl"))
    dev = load_items(os.path.join(ROOT, "jeff-dev", "dev-v2.jsonl"))
    base = [d for d in dev if d["part"] == "base"]
    wide = [d for d in dev if d["part"] == "wide"]
    bad = check("dev (all) vs exam", dev, exam, a.max)
    bad |= check("wide vs exam", wide, exam, a.max)
    bad |= check("wide vs base dev items", wide, base, a.max)
    print("FAIL" if bad else f"OK (max allowed {a.max})")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
