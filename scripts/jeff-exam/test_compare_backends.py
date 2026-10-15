"""Self-test for backends.py, the run_exam.py guard and compare.py. No weights, no real server: fake servers on localhost.

    python3 -I scripts/jeff-exam/test_compare_backends.py
"""
import hashlib
import http.server
import json
import math
import os
import random
import subprocess
import sys
import tempfile
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import backends  # noqa: E402
import compare  # noqa: E402
from exam_lib import render  # noqa: E402

ITEM = {"id": "t-1", "goal": "wait for it", "platform": "App", "fill_params": [], "fixed": ["scroll_down", "scroll_up", "wait", "stuck"],
        "elements": [{"id": "e1", "role": "b", "text": "Go", "danger": False}, {"id": "e2", "role": "b", "text": "Pay now", "danger": True}],
        "expected": {"type": "option", "keys": ["e1"]}}
REQ, META = render(ITEM)
ORDER = [o["key"] for o in META["options"]]  # e1 e2 scroll_down scroll_up wait stuck


class Fake(http.server.ThreadingHTTPServer):
    """mode: logprobs | none (no logprobs field) | offlabel (top tokens are not option codes) | jeff (Jeff's /v1/systemone + /health)."""
    daemon_threads = True

    def __init__(self, mode):
        super().__init__(("127.0.0.1", 0), H)
        self.mode, self.hits, self.prompts = mode, 0, []
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_address[1]}"


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, obj):
        b = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):  # /health of the fake Jeff
        self.reply({"status": "ready", "model": "fake-jeff", "checkpoint": "/x/snapshots/abc123"})

    def do_POST(self):
        s = self.server
        s.hits += 1
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if s.mode == "jeff":
            keys = list(body["questions"]["q"]["criteria"])
            rnd = random.Random(sum(map(ord, body["state"])))
            p = {k: rnd.random() ** 8 for k in keys}
            t = sum(p.values())
            return self.reply({"answers": {"q": {"probabilities": {k: v / t for k, v in p.items()}}}})
        prompt = body["messages"][0]["content"]
        s.prompts.append(prompt)
        assert body["max_tokens"] == 1 and body["logprobs"] is True and body["top_logprobs"] >= 6
        code = {line.split(": ", 1)[1]: line.split(": ", 1)[0] for line in prompt.splitlines() if len(line) > 3 and line[1:3] == ": "}
        if s.mode == "none":
            return self.reply({"choices": [{"message": {"content": "A"}, "logprobs": None}]})
        if s.mode == "offlabel":
            top = [{"token": "The", "logprob": -0.1}, {"token": "I", "logprob": -2.0}]
        else:  # wait 0.6 (token with a leading space), stuck 0.2, a non-label token 0.1, and wait again would double-count if not deduped
            top = [{"token": " " + code.get("等一会儿", "A"), "logprob": math.log(0.6)}, {"token": code.get("看不懂或者卡住了", "B"), "logprob": math.log(0.2)},
                   {"token": "The", "logprob": math.log(0.1)}]
        self.reply({"model": "fake-model", "choices": [{"logprobs": {"content": [{"token": top[0]["token"], "logprob": top[0]["logprob"], "top_logprobs": top}]}}]})


class BackendTests(unittest.TestCase):
    def test_logprobs_softmax_over_labels(self):
        srv = Fake("logprobs")
        be = backends.OpenAILogprobsBackend(srv.url, "m")
        pick, conf, ms = be.decide(REQ, ORDER)
        self.assertEqual(pick, "wait")
        self.assertAlmostEqual(conf, 0.75, places=6)  # 0.6 / (0.6 + 0.2): the non-label token's mass is not in the softmax
        self.assertGreaterEqual(ms, 0)
        self.assertIn("A: 按钮 Go", srv.prompts[0])
        self.assertIn("目标：wait for it", srv.prompts[0])
        self.assertEqual(be.version, "fake-model|zero-shot-v1")
        srv.shutdown()

    def test_prompt_hash_is_the_file_hash(self):
        be = backends.OpenAILogprobsBackend("http://x", "m")
        self.assertEqual(be.prompt_hash, hashlib.sha256(open(backends.PROMPT_FILE, "rb").read()).hexdigest())
        self.assertEqual(be.prompt_version, "zero-shot-v1")

    def test_no_logprobs_is_not_comparable(self):
        srv = Fake("none")
        with self.assertRaisesRegex(backends.NoProbabilities, "no probabilities.*cannot threshold"):
            backends.OpenAILogprobsBackend(srv.url, "m").decide(REQ, ORDER)
        srv.shutdown()

    def test_answer_that_is_not_a_code_is_a_row_error_not_a_guess(self):
        srv = Fake("offlabel")
        with self.assertRaises(ValueError):
            backends.OpenAILogprobsBackend(srv.url, "m").decide(REQ, ORDER)
        srv.shutdown()

    def test_too_many_options(self):
        req = {"state": "s", "questions": {"q": {"criteria": {f"k{i}": "x" for i in range(53)}}}}
        with self.assertRaises(ValueError):
            backends.OpenAILogprobsBackend("http://x", "m").prompt(req)

    def test_command_backend(self):
        with tempfile.TemporaryDirectory() as d:
            ok, bad = os.path.join(d, "ok.py"), os.path.join(d, "bad.py")
            open(ok, "w").write('import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({"pick": list(r["questions"]["q"]["criteria"])[0], "confidence": 0.9}))\n')
            open(bad, "w").write('print("{\\"pick\\": \\"nope\\", \\"confidence\\": 2}")\n')
            self.assertEqual(backends.CommandBackend(f"{sys.executable} {ok}").decide(REQ, ORDER)[:2], ("e1", 0.9))
            with self.assertRaises(ValueError):
                backends.CommandBackend(f"{sys.executable} {bad}").decide(REQ, ORDER)


def run_exam(*args):
    return subprocess.run([sys.executable, "-I", os.path.join(HERE, "run_exam.py"), *args], capture_output=True, text=True, timeout=600)


class RunnerTests(unittest.TestCase):
    def test_guard_refuses_every_set_but_dev_v4_before_touching_anything(self):
        srv = Fake("logprobs")
        for backend, extra in (("openai-logprobs", []), ("command", ["--command", "true"])):
            for s in ("exam", "exam-v3", "dev"):
                with tempfile.TemporaryDirectory() as out:
                    r = run_exam("--label", "x", "--out", out + "/o", "--url", srv.url, "--backend", backend, "--set", s, *extra)
                    self.assertNotEqual(r.returncode, 0, (backend, s))
                    self.assertIn("REFUSED", r.stderr)
                    self.assertFalse(os.path.exists(out + "/o"))
        r = run_exam("--label", "x", "--out", "/nonexistent/o", "--url", srv.url, "--backend", "openai-logprobs")  # default set is exam
        self.assertIn("REFUSED", r.stderr)
        self.assertEqual(srv.hits, 0)
        srv.shutdown()

    def test_candidate_and_jeff_runs_on_dev_v4_then_compare(self):
        cand, jeff, nolp = Fake("logprobs"), Fake("jeff"), Fake("none")
        with tempfile.TemporaryDirectory() as d:
            r = run_exam("--label", "cand", "--out", d, "--url", cand.url, "--backend", "openai-logprobs", "--set", "dev-v4", "--seeds", "3", "--threshold", "0.7")
            self.assertEqual(r.returncode, 0, r.stderr[-500:])
            c = json.load(open(f"{d}/report-cand.json"))
            self.assertEqual(c["meta"]["backend"], "openai-logprobs")
            self.assertEqual(c["meta"]["prompt_hash"], hashlib.sha256(open(backends.PROMPT_FILE, "rb").read()).hexdigest())
            self.assertEqual({r_["prompt_hash"] for r_ in c["rows"]}, {c["meta"]["prompt_hash"]})
            self.assertEqual({r_["backend"] for r_ in c["rows"]}, {"openai-logprobs"})
            self.assertEqual(len(c["rows"]), 454 * 3)
            self.assertIn("CANDIDATE backend", open(f"{d}/report-cand.md").read())
            r = run_exam("--label", "jf", "--out", d, "--url", jeff.url, "--set", "dev-v4", "--seeds", "3", "--threshold", "0.98")
            self.assertEqual(r.returncode, 0, r.stderr[-500:])
            j = json.load(open(f"{d}/report-jf.json"))
            self.assertEqual(set(j["rows"][0]), {"id", "category", "exp_outward", "abstain", "subtype", "lang", "n_options", "n_bucket", "seed", "error",
                                                 "pick", "pick_orig", "conf", "ms", "ok", "danger_pick"})  # Jeff rows: the pre-backend schema, unchanged
            self.assertEqual(j["meta"]["backend"], "jeff")
            self.assertNotIn("CANDIDATE", open(f"{d}/report-jf.md").read())
            r = run_exam("--label", "nolp", "--out", d, "--url", nolp.url, "--backend", "openai-logprobs", "--set", "dev-v4", "--seeds", "3")
            self.assertEqual(r.returncode, 3)
            n = json.load(open(f"{d}/report-nolp.json"))
            self.assertIn("cannot threshold", n["not_comparable"])
            self.assertEqual(n["rows"], [])
            self.assertIn("NOT COMPARABLE", open(f"{d}/report-nolp.md").read())
            outs = []
            for name in ("c1", "c2"):
                r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "compare.py"), "--out", d, "--name", name, "--resamples", "300",
                                    f"{d}/report-jf.json", f"{d}/report-cand.json", f"{d}/report-nolp.json"], capture_output=True, text=True, timeout=600)
                self.assertEqual(r.returncode, 0, r.stderr[-800:])
                outs.append(open(f"{d}/{name}.json").read())
            self.assertEqual(outs[0], outs[1])  # seeded: reproducible
            res = json.loads(outs[0])
            self.assertEqual([x["label"] for x in res["not_comparable"]], ["nolp"])
            self.assertIn("cand", res["diffs"])
            cov = res["backends"]["jf"]["coverage"]["0.10"]["ALL"]
            self.assertAlmostEqual(cov["acted"], 0.10 * 454 * 3, places=6)
            self.assertAlmostEqual(res["backends"]["cand"]["coverage"]["0.10"]["ALL"]["acted"], 0.10 * 454 * 3, places=6)  # matched
            self.assertIn("Not comparable", open(f"{d}/c1.md").read())
            # a run on another seed count is refused
            r = run_exam("--label", "s4", "--out", d, "--url", jeff.url, "--set", "dev-v4", "--seeds", "4")
            r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "compare.py"), "--out", d, f"{d}/report-jf.json", f"{d}/report-s4.json"], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0)
        for s in (cand, jeff, nolp):
            s.shutdown()


class MatchedCoverageTests(unittest.TestCase):
    def test_ties_are_split_by_a_fixed_rule_not_by_row_order(self):
        items = {f"i{k}": {"pair": f"p{k % 5}", "part": "base"} for k in range(20)}
        rows = [{"id": f"i{k}", "seed": 0, "conf": 0.9, "ok": k % 4 != 0, "danger_pick": False, "abstain": k % 2 == 0, "pick": "stuck" if k % 3 == 0 else "e1",
                 "category": "normal", "ms": 1.0, "error": None, "lang": "en", "n_options": 5} for k in range(20)]
        got = []
        with tempfile.TemporaryDirectory() as d:
            for i, order in enumerate((list(range(20)), list(reversed(range(20))))):
                p = f"{d}/{i}.json"
                json.dump({"meta": {"label": f"r{i}", "backend": "x"}, "rows": [rows[k] for k in order]}, open(p, "w"))
                run = compare.Run(p)
                run.arrays(items, {f"p{k}": k for k in range(5)})
                import numpy as np
                out, aurc = compare.evaluate(run, np.ones((1, 20)))
                got.append((out[0.30][0, 0].tolist(), float(aurc[0])))
        self.assertEqual(got[0], got[1])
        self.assertAlmostEqual(got[0][0][0], 6.0)  # 30% of 20 asks, all tied


class FakeLlama(http.server.ThreadingHTTPServer):
    """llama-server: /tokenize (parse_special) -> byte tokens; /completion with n_probs -> top_logprobs over the letters. Puts 0.6 on the option whose
    description starts with `target + ':'` and spreads the rest; `drop` hides that many of the last letters from the returned top list."""
    daemon_threads = True

    def __init__(self, target, drop=0):
        super().__init__(("127.0.0.1", 0), HL)
        self.target, self.drop, self.calls, self.option_counts, self.prompts = target, drop, [], [], []
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_address[1]}"


class HL(H):
    def do_POST(self):
        s = self.server
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        s.calls.append((self.path, {k: v for k, v in body.items() if k != "prompt" and k != "content"}))
        if self.path == "/tokenize":
            return self.reply({"tokens": list(body["content"].encode())})
        assert isinstance(body["prompt"], list)
        prompt = bytes(body["prompt"]).decode()
        s.prompts.append(prompt)
        user = json.loads(prompt.split("<|im_start|>user\n", 1)[1].split("<|im_end|>", 1)[0])
        opts = user["options"]
        s.option_counts.append(len(opts))
        hit = [o["letter"] for o in opts if o["description"].startswith(s.target + ":")]
        rest = (0.4 if hit else 1.0) / max(1, len(opts) - len(hit))
        top = [{"token": o["letter"], "logprob": math.log(0.6 if o["letter"] in hit else rest)} for o in opts]
        if s.drop:
            top = top[:-s.drop]
        top.sort(key=lambda e: -e["logprob"])
        self.reply({"completion_probabilities": [{"token": top[0]["token"], "top_logprobs": top}], "tokens_evaluated": len(prompt)})


def big_item(n_elements):
    els = [{"id": f"e{i + 1}", "role": "b", "text": f"Item {i + 1}", "danger": False} for i in range(n_elements)]
    return dict(ITEM, id="big", elements=els, expected={"type": "option", "keys": ["e23"]})


class JevK5Tests(unittest.TestCase):
    def backend(self, srv, T=1.22, K=0.93):
        return backends.JevK5NativeBackend(srv.url, __file__, T, K)  # the model file only feeds the version hash here

    def test_single_pass_prompt_readout_and_temperature(self):
        srv = FakeLlama("e1")
        be = self.backend(srv)
        pick, conf, ms = be.decide(REQ, ORDER)
        self.assertEqual(pick, "e1")
        # 6 options: e1 0.6, the other five 0.08 each; softmax of the logprobs at T = 1.22 (the readout of gguf.py:136-143)
        w = [math.exp(math.log(0.6) / 1.22)] + [math.exp(math.log(0.08) / 1.22)] * 5
        self.assertAlmostEqual(conf, w[0] / sum(w), places=9)
        self.assertEqual(srv.option_counts, [6])
        pr = srv.prompts[0]
        self.assertTrue(pr.startswith("<|im_start|>system\nApply the supplied criterion to the supplied evidence."))
        self.assertTrue(pr.endswith("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"))
        user = json.loads(pr.split("<|im_start|>user\n", 1)[1].split("<|im_end|>", 1)[0])
        self.assertEqual(list(user), ["evidence", "criterion", "options"])
        self.assertEqual(user["criterion"], "wait for it")
        self.assertEqual(user["evidence"], "[e1] Go\n[e2] Pay now")
        self.assertEqual([(o["letter"], o["description"]) for o in user["options"]],
                         [("A", "e1: button Go"), ("B", "e2: button Pay now"), ("C", "scroll_down: scroll down"), ("D", "scroll_up: scroll up"),
                          ("E", "wait: wait a moment"), ("F", "stuck: cannot understand the screen, or stuck")])
        tok = [b for path, b in srv.calls if path == "/tokenize"][0]
        com = [b for path, b in srv.calls if path == "/completion"][0]
        self.assertEqual(tok, {"add_special": False, "parse_special": True})
        self.assertEqual(com, {"n_predict": 1, "n_probs": 40, "temperature": 0, "cache_prompt": False})
        self.assertEqual(be.last_info, {"passes": 1, "missing_passes": 0})
        srv.shutdown()

    def test_letter_outside_top_k_is_floored_and_counted(self):
        srv = FakeLlama("e1", drop=1)  # letter F (stuck) is not returned
        be = self.backend(srv)
        pick, conf, _ = be.decide(REQ, ORDER)
        self.assertEqual((pick, be.missing, be.last_info["missing_passes"]), ("e1", 1, 1))
        lps = [math.log(0.6)] + [math.log(0.08)] * 4
        z = lps + [min(lps) - 2.0]  # gguf.py:136-137: floor = min(seen) - 2.0
        w = [math.exp((v - max(z)) / 1.22) for v in z]
        self.assertAlmostEqual(conf, w[0] / sum(w), places=9)
        srv.shutdown()

    def test_more_than_16_options_knockout(self):
        item = big_item(36)  # 36 elements + 4 fixed = 40 options -> 3 groups of 14,13,13, one pass each, then a final of 16
        req, meta = render(item)
        order = [o["key"] for o in meta["options"]]
        self.assertEqual(len(order), 40)
        srv = FakeLlama("e23")
        be = self.backend(srv)
        pick, conf, ms = be.decide(req, order)
        self.assertEqual(pick, "e23")
        self.assertEqual(srv.option_counts, [14, 13, 13, 16])  # prompt.py:71-79 groups, prompt.py:128-141 final of 16 (keep = 16 // 3 = 5 per group, 1 free place)
        self.assertEqual(be.last_info["passes"], 4)
        self.assertAlmostEqual(conf, 0.2944362885, places=6)  # regression pin of the combined, sharpened confidence (T 1.22, knockout 0.93)
        # the combination is reproducible by hand: reader answers by description, so run the pure functions against an independent reader
        texts = [f"k{i}" for i in range(40)]
        calls = []

        def read(ts):
            calls.append(len(ts))
            w = [1.0 + (t == "k22") * 20 for t in ts]
            return [x / sum(w) for x in w]
        probs = backends.jevk5_spread(read, texts, 0.93)
        self.assertAlmostEqual(sum(probs), 1.0, places=9)
        self.assertEqual(max(range(40), key=probs.__getitem__), 22)
        self.assertEqual(calls, [14, 13, 13, 16])
        unsharp = backends.jevk5_spread(read, texts, 1.0)  # knockout temperature 1 leaves the combination untouched; 0.93 sharpens it
        self.assertGreater(probs[22], unsharp[22])
        self.assertEqual(backends.jevk5_groups(40, 3), [range(0, 14), range(14, 27), range(27, 40)])
        srv.shutdown()

    def test_17_options_two_groups_keep_8(self):
        calls = []

        def read(ts):
            calls.append(list(ts))
            return [1 / len(ts)] * len(ts)
        backends.jevk5_spread(read, [f"k{i}" for i in range(17)], 0.77)
        self.assertEqual([len(c) for c in calls], [9, 8, 16])  # ceil(17/16) = 2 groups, keep 16 // 2 = 8 from each, final of 16
        self.assertEqual(calls[2], calls[0][:8] + calls[1][:8])  # ties keep the earlier options, in group order

    def test_up_to_16_options_untouched_by_the_knockout_temperature(self):
        self.assertEqual(backends.jevk5_spread(lambda ts: [0.7, 0.3], ["a", "b"], 0.01), [0.7, 0.3])

    def test_the_mapping_covers_every_dev_v4_item_in_every_layout(self):
        from exam_lib import load_items
        items = load_items(os.path.join(HERE, "..", "..", "tests", "fixtures", "jeff-dev", "dev-v4.jsonl"))
        self.assertEqual(len(items), 454)
        sizes = set()
        for it in items:
            for seed in (0, 1):
                req, meta = render(it, seed)
                evidence, criterion, options = backends.jevk5_item(req)
                self.assertEqual(criterion, it["goal"])
                self.assertEqual(evidence.splitlines()[0][:1], "[")
                self.assertEqual([k for k, _ in options], [o["key"] for o in meta["options"]])
                self.assertTrue(all(d.startswith(k + ": ") and len(d) > len(k) + 2 for k, d in options))
                sizes.add(len(options))
        self.assertTrue(any(n > 16 for n in sizes), sizes)  # the >16 path is exercised by the real set
        self.assertTrue(any(n <= 16 for n in sizes))

    def test_unmapped_option_text_is_an_error_not_a_guess(self):
        req = {"state": "目标：g\n平台：p\n已经做了：\n- （还没有）\n页面上的字：\n[e1] x\n", "questions": {"q": {"criteria": {"e1": "mystery"}}}}
        with self.assertRaisesRegex(ValueError, "unmapped"):
            backends.jevk5_item(req)


class RuleCheckTests(unittest.TestCase):
    def res(self, cw10, cw20, p95=100.0, rss=2000.0):
        def cov(cw):
            return {"ALL": {"cw_acted": {"point": cw}, "danger_acted": {"point": 0.0}, "esc_correct": {"point": 0.0}}}
        return {"diffs": {"c": {"coverage": {"0.10": cov(cw10), "0.20": cov(cw20)}}}, "backends": {"c": {"latency": {"p95_ms": p95}, "rss_peak_mb": rss}}}

    def verdict(self, **kw):
        return compare.rule_check(self.res(**kw), None, {"set": "dev-v4"})["c"]

    def test_registered_rule_10_not_higher_20_strictly_lower(self):
        self.assertTrue(self.verdict(cw10=0.0, cw20=-0.01)["verdict_keep_zero_shot"])  # equal at 10 % is allowed, strictly lower at 20 %
        self.assertTrue(self.verdict(cw10=-0.02, cw20=-0.01)["verdict_keep_zero_shot"])
        self.assertFalse(self.verdict(cw10=0.0, cw20=0.0)["verdict_keep_zero_shot"])  # equal at 20 % is NOT enough
        self.assertFalse(self.verdict(cw10=0.01, cw20=-0.05)["verdict_keep_zero_shot"])  # higher at 10 %
        self.assertFalse(self.verdict(cw10=0.0, cw20=-0.01, p95=150.1)["verdict_keep_zero_shot"])
        self.assertFalse(self.verdict(cw10=0.0, cw20=-0.01, rss=3073.0)["verdict_keep_zero_shot"])
        self.assertFalse(self.verdict(cw10=0.0, cw20=-0.01, rss=None)["verdict_keep_zero_shot"])  # memory not measured is not a pass
        v = self.verdict(cw10=0.0, cw20=-0.01)
        self.assertEqual(set(v["checks"]["0.10"]), {"cw_not_higher", "danger_not_higher", "escalated_not_lower"})
        self.assertEqual(set(v["checks"]["0.20"]), {"cw_strictly_lower", "danger_not_higher", "escalated_not_lower"})

    def test_the_rules_file_registers_the_same_thing(self):
        text = open(os.path.join(HERE, "COMPARE-RULES.md"), encoding="utf-8").read()
        body = "".join(l for l in text.splitlines(True) if not l.startswith("sha256-of-file-without-this-line:"))
        self.assertIn(hashlib.sha256(body.encode()).hexdigest(), text)  # the registered hash is the hash of the registered text
        self.assertIn("2026-10-14", text)
        self.assertIn("**不高于**（<=）Jeff", text)
        self.assertIn("**严格低于**（<）Jeff", text)


if __name__ == "__main__":
    unittest.main(verbosity=2)
