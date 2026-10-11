#![cfg(unix)]
use dct_explore::hands::{By, Hands, HelpAnswer, Params, StepReq};
use serde_json::json;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn run_step_sends_the_closed_vocabulary_step_and_reads_help() {
    let dir = tempfile::tempdir().unwrap();
    let h = dct_dco::fake::serve(dir.path(), TOKEN, |tool, args| match tool {
        "run_step" => {
            assert_eq!(args["window"]["window_id"], 4100);
            assert_eq!(args["step"]["action"], "tap_by_intent");
            assert_eq!(args["step"]["arg"], "Post");
            (
                json!({"run_id": "r9", "status": "needs_help", "at_step": 1, "learned": [],
                    "help": {"snapshot_id": "s2", "intent": "Post", "question": "两个 Post",
                             "elements": [{"id": "e1", "text": "Post", "role": "button", "source": "ocr",
                                           "frac": {"x": 0.1, "y": 0.5, "w": 0.05, "h": 0.03}}]}}),
                false,
                Some("AAAA".into()),
            )
        }
        "answer_run" => {
            assert_eq!(args["element_id"], "e1");
            assert_eq!(args["by"], "model");
            (
                json!({"run_id": "r9", "status": "done", "at_step": 1, "learned": [{"key": "k1"}]}),
                false,
                None,
            )
        }
        t => panic!("没想到会调 {t}"),
    });
    let mut c = dct_dco::DcoClient::connect_with_timeout(dir.path(), dct_dco::RUN_TIMEOUT).unwrap();
    let r = c
        .run_step(
            4100,
            &StepReq {
                action: "tap_by_intent".into(),
                arg: "Post".into(),
                done_when: None,
            },
            &Params::new(),
            &[],
        )
        .unwrap();
    assert_eq!(r.status, "needs_help");
    let help = r.help.unwrap();
    assert_eq!(help.elements[0].frac.x, 0.1);
    assert_eq!(help.picture_png.as_deref(), Some("AAAA"));
    let done = c
        .answer_run(
            "r9",
            &HelpAnswer::Element {
                id: "e1".into(),
                by: By::Model,
                name: None,
                dismiss: false,
            },
        )
        .unwrap();
    assert_eq!(done.learned_keys, vec!["k1".to_string()]);
    drop(c);
    h.join().unwrap();
}

#[test]
fn look_asks_for_the_scene_hash() {
    let dir = tempfile::tempdir().unwrap();
    let h = dct_dco::fake::serve(dir.path(), TOKEN, |tool, args| {
        assert_eq!(tool, "see");
        assert_eq!(args["scene_hash"], true);
        (
            json!({"snapshot_id": "s1", "observation_id": "obs-1", "scene_hash": "abcd", "elements": [
            {"id": "e3", "text": "What's happening?", "role": "text", "source": "ocr", "frac": {"x": 0.3, "y": 0.2, "w": 0.3, "h": 0.04}}]}),
            false,
            None,
        )
    });
    let mut c = dct_dco::DcoClient::connect(dir.path()).unwrap();
    let l = c.look(4100, false).unwrap();
    assert_eq!(l.scene_hash.as_deref(), Some("abcd"));
    assert_eq!(l.elements[0].text, "What's happening?");
    drop(c);
    h.join().unwrap();
}

#[test]
fn procedure_windows_and_confirm_calls_have_the_expected_shape() {
    let dir = tempfile::tempdir().unwrap();
    let h = dct_dco::fake::serve(dir.path(), TOKEN, |tool, args| match tool {
        "list_windows" => (
            json!({"windows": [{"window_id": 7, "app": "Google Chrome", "title": "X", "bundle_id": "c"}]}),
            false,
            None,
        ),
        "run_procedure" => {
            assert_eq!(args["stop_before_step"], 5);
            assert_eq!(args["params"]["text"], "hi");
            (
                json!({"run_id": "r1", "status": "paused", "at_step": 5}),
                false,
                None,
            )
        }
        "answer_run" => {
            assert_eq!(args["none"], true);
            (
                json!({"run_id": "r1", "status": "failed", "at_step": 5, "note": "n"}),
                false,
                None,
            )
        }
        "cancel_run" => {
            assert_eq!(args["run_id"], "r1");
            (json!({"ok": true}), false, None)
        }
        "confirm_learned" => {
            assert_eq!(args["accept"], json!(["k1"]));
            assert_eq!(args["reject"], json!([]));
            (json!({"ok": true}), false, None)
        }
        t => panic!("没想到会调 {t}"),
    });
    let mut c = dct_dco::DcoClient::connect(dir.path()).unwrap();
    let w = c.list_windows().unwrap();
    assert_eq!((w[0].window_id, w[0].app.as_str()), (7, "Google Chrome"));
    let mut p = Params::new();
    p.insert("text".into(), "hi".into());
    let r = c
        .run_procedure(7, &json!({"name": "x"}), &p, &[], Some(5))
        .unwrap();
    assert_eq!((r.status.as_str(), r.at_step), ("paused", 5));
    assert_eq!(
        c.answer_run("r1", &HelpAnswer::NotThere)
            .unwrap()
            .note
            .as_deref(),
        Some("n")
    );
    c.cancel_run("r1").unwrap();
    c.confirm_learned(&["k1".to_string()], &[]).unwrap();
    drop(c);
    h.join().unwrap();
}
