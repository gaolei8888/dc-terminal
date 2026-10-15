"""Shared by build_exam.py and run_exam.py: how an exam item becomes the request dct really sends to Jeff.

Mirrors crates/dct-explore/src/options.rs (state_text, all_options, question) and src/judge/jev.rs
(request_body, read_choice): a Chinese header, "[eN] text" page lines, options "按钮 x" / "文字 x",
fill_<param>_<eN>, then the fixed actions. Pure stdlib; run with `python3 -I`.
"""
import hashlib
import json
import random
import zlib

INSTRUCTIONS = "Pick the single next action that best moves toward the goal."
FIXED = {"scroll_down": "向下滚动", "scroll_up": "向上滚动", "wait": "等一会儿", "stuck": "看不懂或者卡住了"}
ABSTAIN_KEY = "stuck"


def sha256_file(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def load_items(path):
    return [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]


def derive_options(elements, fill_params, fixed):
    """Default-order options: [{key, text, kind, danger}] from the page elements (role b/t/i/s; s is not tappable)."""
    opts = []
    for e in elements:
        if e["role"] == "s":
            continue
        kind = "按钮" if e["role"] == "b" else "文字"
        opts.append({"key": e["id"], "text": f"{kind} {e['text']}", "danger": e["danger"]})
    for p in fill_params:
        for e in elements:
            if e["role"] == "i":
                opts.append({"key": f"fill_{p}_{e['id']}", "text": f"在「{e['text']}」里填 {{{p}}}", "danger": False})
    for k in fixed:
        opts.append({"key": k, "text": FIXED[k], "danger": False})
    return opts


def _remap(key, idmap):
    """Map an eN / fill_p_eN key through the (old id -> new id) table; fixed keys are unchanged."""
    if key in idmap:
        return idmap[key]
    if key.startswith("fill_"):
        head, _, old = key.rpartition("_")
        return f"{head}_{idmap[old]}"
    return key


def render(item, seed=0):
    """Item + layout seed -> (request dict, meta). Seed 0 is the natural reading order; seed >= 1 permutes the
    page elements (and renumbers them e1..en), the same screen laid out differently.
    meta: {'options': [{key, orig, danger}] in sent order, 'expected': set of sent keys (empty set = abstain)}."""
    els = [dict(e) for e in item["elements"]]
    if seed:
        random.Random(zlib.crc32(f"{item['id']}:{seed}".encode())).shuffle(els)
    idmap = {e["id"]: f"e{i + 1}" for i, e in enumerate(els)}
    new = [dict(e, id=idmap[e["id"]]) for e in els]
    # default option order for the *new* layout, each tagged with the original key it came from
    inv = {v: k for k, v in idmap.items()}
    opts = derive_options(new, item["fill_params"], item["fixed"])
    for o in opts:
        o["orig"] = _remap(o["key"], inv)
    state = (f"目标：{item['goal']}\n平台：{item['platform']}\n已经做了：\n- （还没有）\n页面上的字：\n"
             + "".join(f"[{e['id']}] {e['text']}\n" for e in new))
    req = {"state": state, "questions": {"q": {"type": "choice", "instructions": INSTRUCTIONS,
                                              "criteria": {o["key"]: o["text"] for o in opts}}}}
    exp = {o["key"] for o in opts if o["orig"] in item["expected"]["keys"]}
    return req, {"options": opts, "expected": exp}


def pick_of(probs, order):
    """dct's read_choice: normalise, take the largest, ties go to the earlier option. -> (key, confidence)."""
    total = sum(probs[k] for k in order) or 1.0
    best = max(order, key=lambda k: probs[k])  # max() keeps the first of equals
    return best, probs[best] / total
