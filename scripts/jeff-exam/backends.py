"""Decision backends for run_exam.py. Each one turns the request dct would send (exam_lib.render) into (pick, confidence, ms).

    jeff             POST /v1/systemone on jeff-serve; the confidence is dct's read_choice over its probabilities (unchanged behaviour).
    openai-logprobs  any OpenAI-compatible /v1/chat/completions server that returns logprobs (llama.cpp server, mlx-lm server, ...). Zero-shot:
                     the screen and options are rendered into prompt-zero-shot-v1.txt, each option gets a ONE-CHARACTER code (A-Z, a-z) so its
                     first token is unambiguous (e1 / e10 would share a first token), the model is asked for the code only, and the probabilities
                     of the first generated token over the codes (softmax over those labels) are the confidence. No logprobs -> NoProbabilities.
    jevk5-native     JevK5 (Qwen3.5 2B/4B GGUF) through llama-server, the prompt and the readout reproduced from the JevK5 sources (read as text, never run).
    command          a user executable: request JSON on stdin, {"pick": key, "confidence": 0..1} on stdout. For candidates with an odd interface.

The API key for openai-logprobs is read from $OPENAI_API_KEY only (never argv). Pure stdlib.
"""
import hashlib
import json
import math
import os
import re
import shlex
import subprocess
import time
import urllib.request

from exam_lib import FIXED, pick_of

PROMPT_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "prompt-zero-shot-v1.txt")
CODES = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
TOP_LOGPROBS = 20


class NoProbabilities(Exception):
    """The backend returned no probabilities: a confidence cannot be computed, so the run cannot be thresholded or compared."""


def prompt_info(path=PROMPT_FILE):
    raw = open(path, "rb").read()
    version = raw.decode("utf-8").splitlines()[0].split(":", 1)[1].strip()
    return version, hashlib.sha256(raw).hexdigest(), raw.decode("utf-8").split("\n---\n", 1)[1]


def post_json(url, body, headers=None, timeout=60):
    r = urllib.request.Request(url, json.dumps(body, ensure_ascii=False).encode(), {"Content-Type": "application/json", **(headers or {})})
    t = time.perf_counter()
    with urllib.request.urlopen(r, timeout=timeout) as resp:
        data = json.load(resp)
    return data, (time.perf_counter() - t) * 1000


class JeffBackend:
    name = "jeff"
    prompt_version = prompt_hash = None

    def __init__(self, url, model, health):
        self.url, self.model = url, model
        self.version = os.path.basename(str(health.get("checkpoint") or "")) or str(health.get("model"))

    def probabilities(self, req):
        data, ms = post_json(self.url.rstrip("/") + "/v1/systemone", dict(req, model=self.model))
        return data["answers"]["q"]["probabilities"], ms

    def decide(self, req, order):
        probs, ms = self.probabilities(req)
        return (*pick_of(probs, order), ms)


class OpenAILogprobsBackend:
    name = "openai-logprobs"

    def __init__(self, url, model, extra=None):
        self.url, self.model, self.extra = url, model, extra or {}
        self.prompt_version, self.prompt_hash, self.template = prompt_info()
        self.version = f"{model}|{self.prompt_version}"

    def prompt(self, req):
        criteria = req["questions"]["q"]["criteria"]
        if len(criteria) > len(CODES):
            raise ValueError(f"{len(criteria)} options but only {len(CODES)} one-character codes")
        lines = "".join(f"{c}: {text}\n" for c, text in zip(CODES, criteria.values()))
        return self.template.replace("{state}", req["state"].rstrip("\n")).replace("{options}", lines.rstrip("\n"))

    def decide(self, req, order):
        body = {"model": self.model, "messages": [{"role": "user", "content": self.prompt(req)}], "max_tokens": 1, "temperature": 0,
                "logprobs": True, "top_logprobs": TOP_LOGPROBS, **self.extra}
        headers = {"Authorization": f"Bearer {os.environ['OPENAI_API_KEY']}"} if os.environ.get("OPENAI_API_KEY") else {}
        data, ms = post_json(self.url.rstrip("/") + "/v1/chat/completions", body, headers)
        if data.get("model"):
            self.version = f"{data['model']}|{self.prompt_version}"
        return (*self.read(data, order), ms)

    @staticmethod
    def read(data, order):
        try:
            content = data["choices"][0]["logprobs"]["content"]
            first = content[0]
        except (KeyError, IndexError, TypeError):
            first = None
        if not first or (not first.get("top_logprobs") and first.get("logprob") is None):
            raise NoProbabilities("no probabilities: the server returned no logprobs (start it with logprobs enabled), cannot threshold")
        seen = {}
        for e in [{"token": first.get("token"), "logprob": first.get("logprob")}] + list(first.get("top_logprobs") or []):
            if e.get("token") is not None and e.get("logprob") is not None:
                seen[e["token"]] = e["logprob"]  # the sampled token is also in top_logprobs: dedupe by token string
        code_of = dict(zip(order, CODES))
        probs = {k: 0.0 for k in order}
        for tok, lp in seen.items():
            for k, c in code_of.items():
                if tok.strip() == c:
                    probs[k] += math.exp(lp)
        if not sum(probs.values()):
            raise ValueError("none of the option codes is among the top first-token logprobs (the model did not answer with a code)")
        return pick_of(probs, order)  # softmax over the option labels = normalised mass


class CommandBackend:
    name = "command"
    prompt_version = prompt_hash = None

    def __init__(self, command):
        self.argv = shlex.split(command)
        exe = self.argv[0] if os.path.isfile(self.argv[0]) else None
        self.version = (hashlib.sha256(open(exe, "rb").read()).hexdigest()[:12] if exe else "?") + "|" + os.path.basename(self.argv[0])

    def decide(self, req, order):
        t = time.perf_counter()
        r = subprocess.run(self.argv, input=json.dumps(req, ensure_ascii=False), capture_output=True, text=True, timeout=60)
        ms = (time.perf_counter() - t) * 1000
        if r.returncode:
            raise RuntimeError(f"command exited {r.returncode}: {r.stderr.strip()[:200]}")
        out = json.loads(r.stdout)
        pick, conf = out.get("pick"), out.get("confidence")
        if pick not in order or isinstance(conf, bool) or not isinstance(conf, (int, float)) or not 0 <= conf <= 1:
            raise ValueError(f"command output must be {{pick: one of the option keys, confidence: 0..1}}, got {r.stdout.strip()[:200]}")
        return pick, float(conf), ms


# ---------------------------------------------------------------------------------------------------------------------------------------------
# jevk5-native. Reproduces, WITHOUT importing or running the JevK5 package, what github.com/allebee/jevk5 @ main does for a llama.cpp GGUF.
# Sources read as raw text (sha256 of the files used): jevk5/prompt.py 322d69ba6abea83602f27b7a5aa0d3e6ab00847fd03ddccd35a28ae5b23acc1a, jevk5/gguf.py 6505de497ce115f0bf85f110cf708fc8535a774f1361b811e3415b3e91a10d03 (main, fetched 2026-10-14); cited below as prompt.py:LINE / gguf.py:LINE.
#
# MAPPING of a dev-v4 item (what Jeff sees, exam_lib.render) onto JevK5's typed choice question (the SAME options, in the SAME order):
#   evidence  = the screen text lines "[e1] text" exactly as in the request's "页面上的字：" block (so the option ids refer to something in the evidence)
#   criterion = the goal ("目标：" line)                    (platform and the "已经做了" line are NOT given: not screen text)
#   options   = every option Jeff sees, in Jeff's order, keys unchanged (e1.., fill_<p>_<e>, scroll_down, scroll_up, wait, stuck),
#               description = "<key>: <plain-language text>" (decision_options, prompt.py:65). The plain language is ENGLISH for every item (JevK5 is
#               English-only): "按钮 X" -> "button X", "文字 X" -> "text X", "在「X」里填 {p}" -> "type {p} into the field X",
#               scroll_down/scroll_up/wait/stuck -> "scroll down" / "scroll up" / "wait a moment" / "cannot understand the screen, or stuck".
#               The element TEXT itself stays in the item's language (zh items stay Chinese); only our own labels are translated.
#   letters   = A..P, at most 16 options per pass (prompt.py:13); more options -> the knockout (below).
# ---------------------------------------------------------------------------------------------------------------------------------------------
JEVK5_LETTERS = "ABCDEFGHIJKLMNOP"  # prompt.py:13
JEVK5_SYSTEM = ("Apply the supplied criterion to the supplied evidence. Choose exactly one listed option. "  # prompt.py:21-24
                "Respond with only its uppercase letter, with no explanation or reasoning.")
JEVK5_CHAT_TEMPLATE = ("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n"  # prompt.py:28-32 (thinking off)
                       "<|im_start|>assistant\n<think>\n\n</think>\n\n")
JEVK5_MISSING_MARGIN = 2.0  # gguf.py:37: a letter outside the returned top-k sits this far below the last returned one
JEVK5_FIXED_EN = {"scroll_down": "scroll down", "scroll_up": "scroll up", "wait": "wait a moment", "stuck": "cannot understand the screen, or stuck"}
_FIXED_ZH = {v: JEVK5_FIXED_EN[k] for k, v in FIXED.items()}
_FILL = re.compile(r"^在「(.*)」里填 \{(.*)\}$", re.S)


def jevk5_prompt_hash():
    return hashlib.sha256(json.dumps([JEVK5_LETTERS, JEVK5_SYSTEM, JEVK5_CHAT_TEMPLATE, sorted(JEVK5_FIXED_EN.items()), "mapping-v1"]).encode()).hexdigest()


def jevk5_groups(n, count):
    """prompt.py:71-79: `count` contiguous runs covering range(n), sizes differing by at most one."""
    base, extra = divmod(n, count)
    runs, start = [], 0
    for g in range(count):
        stop = start + base + (g < extra)
        runs.append(range(start, stop))
        start = stop
    return runs


def jevk5_spread(read, texts, knockout_temperature):
    """prompt.py:82-111 (`spread`, method knockout): <= 16 options: read(texts) untouched; more: _knockout then sharpened by q ** (1 / T), renormalised."""
    if len(texts) <= len(JEVK5_LETTERS):
        return list(read(texts))
    probs = _jevk5_combine(read, texts)
    if knockout_temperature != 1.0:
        probs = [q ** (1 / knockout_temperature) for q in probs]
        total = sum(probs)
        probs = [q / total for q in probs]
    return probs


def _jevk5_combine(read, texts):  # prompt.py:114-124
    if len(texts) <= len(JEVK5_LETTERS):
        return list(read(texts))
    weights = _jevk5_knockout(read, texts)
    total = sum(weights)
    return [w / total for w in weights]


def _jevk5_knockout(read, texts):  # prompt.py:127-151, line for line
    runs = jevk5_groups(len(texts), -(-len(texts) // len(JEVK5_LETTERS)))  # ceil(n / 16) groups of near-equal contiguous blocks
    inner = [list(read([texts[i] for i in run])) for run in runs]  # one pass per group
    inner = [[q / sum(p) for q in p] for p in inner]
    keep = max(1, len(JEVK5_LETTERS) // len(runs))  # top 16 // groups of each group go to the final
    ranked = [sorted(range(len(p)), key=lambda j: -p[j]) for p in inner]  # stable: ties to the earlier option
    chosen = {(g, j) for g, order in enumerate(ranked) for j in order[:keep]}
    rest = sorted(((g, j) for g, order in enumerate(ranked) for j in order[keep:]), key=lambda gj: -inner[gj[0]][gj[1]])
    chosen.update(rest[: max(0, len(JEVK5_LETTERS) - len(chosen))])  # free places: next most likely in any group
    tops = [sorted(j for h, j in chosen if h == g) for g in range(len(runs))]
    final = _jevk5_combine(read, [texts[run[j]] for run, top in zip(runs, tops) for j in top])  # the final pass (<= 16)
    shares, at = [], 0
    for top in tops:
        shares.append(dict(zip(top, final[at: at + len(top)])))
        at += len(top)
    in_final = sum(sum(f.values()) * sum(p[j] for j in f) for p, f in zip(inner, shares))
    weights = []
    for p, f in zip(inner, shares):
        mass = sum(f.values())
        weights += [f[j] * in_final if j in f else mass * q for j, q in enumerate(p)]
    return weights


def jevk5_item(req):
    """(evidence, criterion, [(key, 'key: english description')]) from the request Jeff would get; see the MAPPING comment above."""
    state = req["state"]
    criterion = state.split("\n平台：", 1)[0].removeprefix("目标：")
    evidence = state.split("页面上的字：\n", 1)[1].rstrip("\n")
    options = []
    for key, text in req["questions"]["q"]["criteria"].items():
        if text.startswith("按钮 "):
            d = "button " + text[3:]
        elif text.startswith("文字 "):
            d = "text " + text[3:]
        elif (m := _FILL.match(text)):
            d = f"type {{{m.group(2)}}} into the field {m.group(1)}"
        elif text in _FIXED_ZH:
            d = _FIXED_ZH[text]
        else:
            raise ValueError(f"unmapped option text {text!r}")
        options.append((key, f"{key}: {d}"))  # decision_options, prompt.py:65
    return evidence, criterion, options


class JevK5NativeBackend:
    """JevK5 read through llama-server's /tokenize + /completion (gguf.py:99-121, 123-147). Nothing is generated; the log-probabilities of the
    answer letters at the first answer position are the readout, renormalised under the file's calibration temperature."""
    name = "jevk5-native"

    def __init__(self, url, model_file, temperature, knockout_temperature, top_k=40, file_sha256=None):
        self.url, self.temperature, self.knockout_temperature, self.top_k = url.rstrip("/"), temperature, knockout_temperature, top_k
        sha = file_sha256 or hashlib.sha256(open(model_file, "rb").read()).hexdigest()
        self.version = f"{os.path.basename(model_file)}|{sha}"
        self.prompt_version = "jevk5-native-v1 (ChatML thinking off, JSON user message, letters A-P, knockout >16)"
        self.prompt_hash = jevk5_prompt_hash()
        self.temperatures = {"temperature": temperature, "knockout_temperature": knockout_temperature}
        self.missing = self.passes = self.decisions = 0  # missing: passes in which an option letter was not among the top_k tokens
        self.last_info = {}

    def _logprobs(self, prompt):  # gguf.py:99-121
        tokens, _ = post_json(self.url + "/tokenize", {"content": prompt, "add_special": False, "parse_special": True}, timeout=600)
        out, _ = post_json(self.url + "/completion", {"prompt": tokens["tokens"], "n_predict": 1, "n_probs": self.top_k, "temperature": 0, "cache_prompt": False}, timeout=600)
        return {e["token"]: e["logprob"] for e in out["completion_probabilities"][0]["top_logprobs"]}

    def decide(self, req, order):
        evidence, criterion, options = jevk5_item(req)
        if [k for k, _ in options] != list(order):
            raise ValueError("option order mismatch")
        passes = missing = 0

        def read(texts):  # gguf.py:129-143: one pass, calibrated distribution over the texts' letters
            nonlocal passes, missing
            user = json.dumps({"evidence": evidence, "criterion": criterion,
                               "options": [{"letter": JEVK5_LETTERS[i], "description": d} for i, d in enumerate(texts)]}, ensure_ascii=False)  # prompt.py:35-44
            seen = self._logprobs(JEVK5_CHAT_TEMPLATE.format(system=JEVK5_SYSTEM, user=user))
            passes += 1
            floor = min(seen.values(), default=0.0) - JEVK5_MISSING_MARGIN
            logprobs = [seen.get(JEVK5_LETTERS[i], floor) for i in range(len(texts))]
            if any(JEVK5_LETTERS[i] not in seen for i in range(len(texts))):
                missing += 1
            top = max(logprobs)
            w = [math.exp((z - top) / self.temperature) for z in logprobs]
            t = sum(w)
            return [x / t for x in w]

        t0 = time.perf_counter()
        probs = jevk5_spread(read, [d for _, d in options], self.knockout_temperature)  # gguf.py:146
        ms = (time.perf_counter() - t0) * 1000
        self.passes, self.missing, self.decisions = self.passes + passes, self.missing + missing, self.decisions + 1
        self.last_info = {"passes": passes, "missing_passes": missing}
        return (*pick_of(dict(zip(order, probs)), order), ms)
