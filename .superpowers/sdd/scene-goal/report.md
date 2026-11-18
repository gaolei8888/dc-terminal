# scene-goal report

Status: done on branch feat/scene-goal (not pushed).

- `dct game scene --goal "<text>"`: trimmed, empty/missing is a plain-Chinese error. Default behaviour and prompt unchanged.
- Prompt gets one extra line with the goal and the `"done": true` hint (LlmVision.with_goal; question() takes Option<&str>).
- `Pick.done` (default false). With a goal, done:true records a step (label goal_reached, no tap) and stops with SceneStop::GoalReached (exit 0). Without a goal done is ignored. x/y/name stay required in the reply.
- steps.jsonl: every record has `goal` (text or null).
- Summary: extra line "大模型一共用了 X.X 秒，走了 N 步。" (sum of model_ms, SceneSummary.model_ms).
- Tests added: parse --goal, prompt with/without goal, done with/without goal, record goal, time line, stop line.
- Mutation (done ignored when goal given): done_stops_with_goal_reached_only_when_a_goal_was_given fails; restored, passes.
- Full workspace tests run 3 times: only unrelated flaky tests failed, a different one each run (daemon web listener, zombie_reaping, dct-srv polling, session recovery); each passes in other runs. Clippy -D warnings clean.
