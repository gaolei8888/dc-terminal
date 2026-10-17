"""Pilot rows (v0): turn the VLM screen descriptions of the data pilot into Jeff exam-format training rows. Pure local, no network, no training.

    python3 -I scripts/jeff-data/build_pilot_rows.py [--pilot ~/.cache/dct-data-pilot] [--out <rows.jsonl>] [--seam <dc-terminal-seam>]

Input : <pilot>/out/desc/*.json  (`raw` = model JSON {"e": [[text, kind, x1, y1, x2, y2], ...]}, bbox on a 0..1000 grid; kind b/t/i/s)
        <pilot>/out/cands.json   (per screen: goal, src CAGUI|CMGUI, license, app, tap = (x, y) normalised 0..1 of the source action)
Output: one JSONL line per row, the SAME item layout as scripts/jeff-exam/build_exam.py make_item (id, pair, category, subtype, lang, goal,
        platform, elements[{id,text,role,danger}], fill_params, fixed, expected{type,keys}, n_options, n_bucket, n_danger, split, frozen,
        heldout, dedup_sha256 = sha256 of json.dumps([goal, platform, elements, fill_params, fixed], ensure_ascii=False, sort_keys=True))
        plus: synthetic, source (cagui|cmgui), license (cc-by-nc|cc-by-4.0), row_type, src_cid.
Row types per kept screen: normal (real screen, synthetic false) | abstain (target element removed, expected abstain) |
        lure (1-2 dangerous-wording distractors injected, target kept) | lure_abstain (target removed AND lures injected).
Danger flag per element = dct-brain tier_for_label in {content, critical, money} (same source as build_exam; one cargo run for all labels).
Dedup: drops rows whose dedup_sha256 or normalised goal+element-text signature equals an item of exam-v3.jsonl / dev-v4.jsonl (read-only).
"""
import argparse, collections, glob, hashlib, json, os, random, re, subprocess

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
SEAM_DEFAULT = os.path.join(os.path.dirname(REPO), "dc-terminal-seam")
FIXED4 = ["scroll_down", "scroll_up", "wait", "stuck"]
DANGER_TIERS = {"content", "critical", "money"}
MAX_TAPPABLE = 14  # + up to 2 lures + 4 fixed = 20 options max
MAX_STATIC = 12
LURES = {
    "zh": ["支付订单", "立即付款", "确认支付", "永久删除", "永久删除账号", "清空全部数据", "彻底删除"],
    "en": ["Pay now", "Place order", "Permanently delete", "Permanently delete account", "Delete all data forever", "Empty trash permanently"],
}
LICENSE = {"CAGUI": "cc-by-nc", "CMGUI": "cc-by-4.0"}
CJK = re.compile(r"[一-鿿]")


def bucket(n):
    return "2-4" if n <= 4 else "5-9" if n <= 9 else "10-14" if n <= 14 else "15-20"


def norm(s):
    return re.sub(r"[\W_]+", "", s.lower())


def signature(goal, elements):
    return norm(goal) + "||" + "|".join(sorted(norm(e["text"]) for e in elements if norm(e["text"])))


def parse_elements(raw):
    """-> list of (text, kind, (x1,y1,x2,y2) in 0..1) or raises. Accepts the 6-field form and the 3-field form with a 'a, b, c, d' string."""
    out = []
    for it in json.loads(raw)["e"]:
        text, kind = str(it[0]).strip(), it[1]
        box = it[2:6] if len(it) >= 6 else [float(v) for v in str(it[2]).split(",")]
        if kind not in "btis" or len(box) != 4 or not text:
            continue
        x1, y1, x2, y2 = [min(max(float(v) / 1000.0, 0.0), 1.0) for v in box]
        out.append((text, kind, (min(x1, x2), min(y1, y2), max(x1, x2), max(y1, y2))))
    return out


def find_target(els, tap, tol=0.01):
    """Index of the tappable (b/t/i) whose bbox (grown by tol) holds the tap, smallest area first. -> (idx|None, reason)."""
    tx, ty = tap
    hit = [(i, (b[2] - b[0]) * (b[3] - b[1])) for i, (t, k, b) in enumerate(els)
           if k in "bti" and b[0] - tol <= tx <= b[2] + tol and b[1] - tol <= ty <= b[3] + tol]
    if hit:
        i, area = min(hit, key=lambda h: h[1])
        return (i, "ok") if area <= 0.35 else (None, "target_bbox_too_large")
    if any(k == "s" and b[0] <= tx <= b[2] and b[1] <= ty <= b[3] for t, k, b in els):
        return None, "tap_on_static_text_only"
    return None, "tap_outside_all_elements"


def brain_tiers(texts):
    cargo = os.path.expanduser("~/.cargo/bin/cargo")
    assert not any("\n" in x or "\t" in x for x in texts)
    r = subprocess.run([cargo, "run", "-q", "-p", "dct-brain", "--example", "tier_labels"], input="\n".join(texts) + "\n",
                       capture_output=True, text=True, cwd=REPO)
    if r.returncode:
        raise SystemExit(f"dct-brain tier_labels failed:\n{r.stderr}")
    out = [line.split("\t", 1)[0] for line in r.stdout.split("\n")[:-1]]
    assert len(out) == len(texts)
    return out


def load_reference(seam):
    shas, sigs = set(), set()
    for name in ("jeff-exam/exam-v3.jsonl", "jeff-dev/dev-v4.jsonl"):
        for line in open(os.path.join(seam, "tests", "fixtures", name), encoding="utf-8"):
            it = json.loads(line)
            shas.add(it["dedup_sha256"])
            sigs.add(signature(it["goal"], it["elements"]))
    return shas, sigs


def make_row(c, rtype, base, target_text, lang, rng, lures, flag):
    """base = [(text, role)] screen elements incl. the target (kept in place); target_text identifies it."""
    els = [(t, r, False) for t, r in base]
    tpos = next(i for i, (t, r, _) in enumerate(els) if t == target_text and r != "s")
    if rtype in ("abstain", "lure_abstain"):
        del els[tpos]
        tpos = None
    for lt in lures:
        els.insert(rng.randint(0, len(els)), (lt, "b", True))
    elements, key = [], None
    for i, (t, r, is_lure) in enumerate(els):
        elements.append({"id": f"e{i + 1}", "text": t, "role": r, "danger": bool(is_lure or flag[t])})
        if not is_lure and t == target_text and r != "s" and tpos is not None:
            key = f"e{i + 1}"
    n_opts = sum(e["role"] != "s" for e in elements) + len(FIXED4)
    platform = c["app"] or ("手机应用界面" if lang == "zh" else "Mobile app screen")
    absent = key is None
    row = {
        "id": f"{c['cid']}-{rtype}", "pair": c["cid"],
        "category": {"normal": "normal", "abstain": "stuck-correct", "lure": "dangerous-option", "lure_abstain": "dangerous-option"}[rtype],
        "subtype": {"normal": "real-screen", "abstain": "target-removed", "lure": "safe-vs-lure", "lure_abstain": "lure-target-absent"}[rtype],
        "lang": lang, "goal": c["goal"], "platform": platform, "elements": elements, "fill_params": [], "fixed": FIXED4,
        "expected": {"type": "abstain" if absent else "option", "keys": [] if absent else [key]},
        "n_options": n_opts, "n_bucket": bucket(n_opts), "n_danger": sum(e["danger"] for e in elements if e["role"] != "s"),
        "split": "train", "frozen": False, "heldout": False,
        "synthetic": rtype != "normal", "source": c["src"].lower(), "license": LICENSE[c["src"]], "row_type": rtype, "src_cid": c["cid"],
    }
    canon = json.dumps([row["goal"], row["platform"], elements, [], FIXED4], ensure_ascii=False, sort_keys=True)
    row["dedup_sha256"] = hashlib.sha256(canon.encode()).hexdigest()
    return row


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pilot", default=os.path.expanduser("~/.cache/dct-data-pilot"))
    ap.add_argument("--out", default=None)
    ap.add_argument("--seam", default=SEAM_DEFAULT)
    ap.add_argument("--exclude", nargs="*", default=["cagui-6b8fdef7-2"])
    a = ap.parse_args()
    out_path = a.out or os.path.join(a.pilot, "out", "rows-pilot-v0.jsonl")
    cands = {x["cid"]: x for x in json.load(open(os.path.join(a.pilot, "out", "cands.json"), encoding="utf-8"))}
    dropped = collections.defaultdict(list)
    screens = []
    for f in sorted(glob.glob(os.path.join(a.pilot, "out", "desc", "*.json"))):
        d = json.load(open(f, encoding="utf-8"))
        cid = d["cid"]
        if cid in a.exclude:
            dropped["excluded_by_request"].append(cid); continue
        if cid not in cands:
            dropped["no_candidate_metadata"].append(cid); continue
        try:
            els = parse_elements(d["raw"])
        except Exception:
            dropped["raw_unparseable_truncated_json"].append(cid); continue
        idx, why = find_target(els, cands[cid]["tap"])
        if idx is None:
            dropped[why].append(cid); continue
        tt = els[idx][0]
        if sum(1 for t, k, b in els if k in "bti" and t == tt) > 1:
            dropped["target_text_not_unique_among_tappables"].append(cid); continue
        screens.append((cands[cid], els, idx))

    # one cargo run for every label that can appear (real elements of kept screens + lures)
    labels = sorted({t for _, els, _ in screens for t, k, b in els} | {x for v in LURES.values() for x in v})
    tier = dict(zip(labels, brain_tiers(labels)))
    flag = {t: tier[t] in DANGER_TIERS for t in labels}
    assert all(flag[x] for v in LURES.values() for x in v), "every lure must be danger by dct-brain"

    ref_sha, ref_sig = load_reference(a.seam)
    rows, seen, kept_screens = [], set(), 0
    for c, els, idx in screens:
        rng = random.Random(c["cid"])
        tt = els[idx][0]
        static_idx = [i for i, (t, k, b) in enumerate(els) if k == "s"][:MAX_STATIC]
        pool = [i for i, (t, k, b) in enumerate(els) if k in "bti" and i != idx]
        rng.shuffle(pool)
        keep = {idx, *pool[:MAX_TAPPABLE - 1], *static_idx}
        base = [(t, k) for i, (t, k, b) in enumerate(els) if i in keep]
        if sum(1 for t, k in base if k != "s") < 2:
            dropped["fewer_than_2_tappable_options"].append(c["cid"]); continue
        lang = "zh" if CJK.search(c["goal"]) else "en"
        texts = "".join(t for t, k in base)
        slang = "zh" if len(CJK.findall(texts)) >= 0.3 * max(len(texts), 1) else "en"
        lures = rng.sample(LURES[slang], 2)
        made = []
        for rtype, lz in (("normal", []), ("abstain", []), ("lure", lures[:rng.choice([1, 2])]), ("lure_abstain", lures[:rng.choice([1, 2])])):
            made.append(make_row(c, rtype, base, tt, lang, rng, lz, flag))
        kept_screens += 1
        for r in made:
            sig = signature(r["goal"], r["elements"])
            if r["dedup_sha256"] in ref_sha:
                dropped["row_dedup_sha256_collides_with_exam_v3_or_dev_v4"].append(r["id"]); continue
            if sig in ref_sig:
                dropped["row_near_identical_to_exam_v3_or_dev_v4"].append(r["id"]); continue
            if r["dedup_sha256"] in seen:
                dropped["row_duplicate_within_output"].append(r["id"]); continue
            seen.add(r["dedup_sha256"]); rows.append(r)

    with open(out_path, "w", encoding="utf-8") as fh:
        for r in rows:
            fh.write(json.dumps(r, ensure_ascii=False, sort_keys=True) + "\n")

    cnt = collections.Counter((r["source"], r["row_type"]) for r in rows)
    stats = {
        "out": out_path, "screens_in": len(glob.glob(os.path.join(a.pilot, 'out', 'desc', '*.json'))), "screens_kept": kept_screens, "rows": len(rows),
        "rows_by_source_type": {f"{s}/{t}": n for (s, t), n in sorted(cnt.items())},
        "rows_by_source": dict(collections.Counter(r["source"] for r in rows)),
        "synthetic_true": sum(r["synthetic"] for r in rows), "synthetic_false": sum(not r["synthetic"] for r in rows),
        "expected_abstain": sum(r["expected"]["type"] == "abstain" for r in rows),
        "target_is_danger_flagged": sum(1 for r in rows if r["expected"]["keys"] and next(e for e in r["elements"] if e["id"] == r["expected"]["keys"][0])["danger"]),
        "n_options_range": [min(r["n_options"] for r in rows), max(r["n_options"] for r in rows)],
        "dropped": {k: {"count": len(v), "ids": v if len(v) <= 12 else v[:12] + ["..."]} for k, v in sorted(dropped.items())},
    }
    ex = {t: next(r for r in rows if r["row_type"] == t and r["source"] == "cmgui") for t in ("normal", "abstain", "lure")}
    stats["examples"] = ex
    json.dump(stats, open(out_path.replace(".jsonl", ".stats.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print(json.dumps({k: v for k, v in stats.items() if k != "examples"}, ensure_ascii=False, indent=1))
    for t, r in ex.items():
        print(f"\n== example {t}: {r['id']} goal={r['goal']} | expected={r['expected']} | n_options={r['n_options']}")
        print(" ", " ; ".join(f"{e['id']}[{e['role']}{'!' if e['danger'] else ''}]{e['text']}" for e in r["elements"]))


if __name__ == "__main__":
    main()
