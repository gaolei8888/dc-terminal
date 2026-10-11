# dct 自己训练 Jeff：攒题、闲时训练、冻结考试、用户点头才换

日期：2026-10-11
前提：`explore-and-learn` 分支已经让 `dct do` 在摸索时把每道答过的题写进 `~/.dct/explore/answers.jsonl`（见 `2026-09-26-dct-judge-design.md` 末尾「实现以后补的几件事」）。
依据：dc-octo `docs/experiments/2026-10-11-jeff-serve-d4.md`（D4，v1.2 真跑）、`docs/experiments/2026-10-11-jeff-v13-adapter-e1.md`（E1，v1.3 adapter 怎么训练、怎么上线）、`docs/experiments/2026-10-11-jeff-social-media-training-plan.md`（训练计划，考试平台用户已同意）。

## 要解决什么

Jeff（0.8B 的「选哪个」小模型）现在零样本用，GUI 题英文七成、中文六成左右，把握不够的都要叫大模型，慢、要花钱。
用户和大模型在 `dct do` 里答过的题，正好是 Jeff 最该学的东西。用户要的是：**dct 自己攒题、自己在电脑闲着时训练、自己考试，考得比现在好才来问一句「要换吗」**；用户点头才换，换坏了一条命令换回去。

## 定下的规矩（不在本设计里再议）

- **dco 永远不训练、不调用任何模型。** 训练、考试、换版本全在 dct 这边（dct 调外部的 `jeff-train` / `jeff-serve`）。
- **危险与否永远靠规则**：dct 的停下规矩和禁点词（`crates/dct-explore/src/gate.rs`）、dco 的按钮分档。绝不看模型的把握，换了新 Jeff 也一样。
- **不打包权重**：模型在 dcv 里登记「允许用哪个版本」（dcv 只记引用和指纹，不存权重），用到时下载。
- **写盘的题里没有用户的文字**：参数值写盘前已经换回 `{名字}`（现在就是这样，见下）。
- **考试平台**：Instagram 和小红书只考不练（用户 2026-10-11 同意）；以后新加的平台也先当考试平台。
- **新版不比现役好就不换**；比现役好也要用户点头。

## 1. 题从哪来：现在磁盘上实际是什么样

### 位置和格式（照现在的代码，v2，任务 1 已做，提交 `202fab9`）

- 训练题：`~/.dct/explore/answers.jsonl`，一行一道题，0600，追加写（`crates/dct-explore/src/trace.rs` 的 `QuestionRow` / `append_question`）。摸索循环先把答过的题留在内存里（`explore/act.rs` 的 `pending_row`），**整次运行结束、结局定了以后**才由 `explore_from` 调 `write_rows` 一次写下。这样每行的 `outcome` / `label` 写下就是定的，不用回头改文件；代价是进程半路被杀掉，这次的题就没了（轨迹本来也只在结束时存）。
- 每次运行的轨迹：`~/.dct/explore/runs/<run_id>/trace.json`（0600，`trace::save`，`src/explore_cli.rs` 在运行结束时存），同目录还有问用户时给他看的编号图 `ask-<n>.jpg`。`run_id` 形如 `x-20261011-1a2b`。轨迹多了 `outcome` 字段（`published` / `declined` / `halted` / `stopped`，和 `explore::Outcome` 一一对应，摸索返回前填上）；旧轨迹没有这个字段，照样能读（读出来是空）。

一行题的字段（前九个是 v1 就有的；v1 的行没有 `v`）：

| 字段 | 内容 |
|---|---|
| `ts_ms` | 写这一行的时间 |
| `run_id`、`platform` | 哪次运行、哪个平台（平台表里的短名，如 `x`、`xiaohongshu`） |
| `state` | 题面，就是发给 Jeff 的那段文字：`目标：…`、`平台：…`、`已经做了：…`、`页面上的字：` 下每个元素一行 `[<元素号>] <字>`（最多 200 行）。模板是中文的，画面上的字是什么语言就是什么语言 |
| `options` | 这一轮**全部**选项的 `[key, 文字]`，按出现顺序。可能多于 20 个；发给 Jeff 的是 `shown` 里那几个 |
| `jeff` | Jeff 当时的 `[key, 把握]`；这个画面上 Jeff 之前选错过的，记的是那次的答案；没问 Jeff 就是 `null` |
| `answer` | 最后照做的那个 key |
| `by` | 谁答的：`jeff` / `model` / `user` |
| `corrects_jeff` | 不是 Jeff 答的、而 Jeff 在这个画面上选过别的（Jeff 错了、被纠正） |
| `v` | 格式版本，现在是 `2`（`trace::QUESTION_ROW_V`） |
| `shown` | 这一轮候选单（`options::shortlist`，最多 20 个）的 key，按发给 Jeff 的顺序，是 `options` 的子集。Jeff 在这个画面上失败过、这一轮没问它时，记的是会发给它的那一份。训练时用 `state` + `shown` 就能原样拼出 Jeff 当时那道题 |
| `step` | 照这个答案做的**第一步**在同一 `run_id` 的 `trace.json` 里 `steps` 的序号（轨迹只追加不删，序号不变）；选「做完了」（`done`）的那道题为 `null` |
| `run_outcome` | 这次运行的结局，和轨迹的 `outcome` 一样 |
| `outcome` | 这一下是不是**确认离目标更近**：`good` / `bad` / `unknown`，定义见下 |
| `label` | 能不能当标准答案，规矩见下 |

key 的样子：元素是 `e<编号>`（如 `e17`），填字是 `fill_<参数名>_<元素>`，固定动作是 `scroll_down`、`scroll_up`、`wait`、`done`、`stuck`。没有纯数字的 key，可以原样当训练题的选项键（线上请求也是这些 key，训练和使用一致）。

### 什么时候会写一行（这决定了一行能说明什么）

都在运行结束时写，下面说的是「哪些答过的题会留下一行」：

- 照做以后到了**这次运行里没见过的新画面**，才记一行；画面没变、或绕回了见过的画面，不记。
- 发布以后选「已经发完了」（`done`）写一行。
- 发布要用户自己点的平台（`wechat_mp`），用户点完发布那一下照样记一行，不管画面变没变；画面没到新画面时这行 `outcome: bad`。
- 重放时 dco 求助、dct 替它答的题，现在**不写**。

### 「确认离目标更近」怎么定（`trace::grade`，只用循环自己知道的事）

- **good**：dco 回 done（或 `already_done`）、照做后到了**这次运行没见过的新画面**（dco 的画面指纹核对）、这一步在**最终**轨迹里还是 `ok: true`（没被后来的绕圈改成 `ok: false`，即还在「从头走到最后那个画面」的路上），**而且这次运行发出去了**（`run_outcome: published`）。`done` 那道题只在发出去时才会记，所以总是 `good`。
- **bad**：确认没起作用——照做后没到新画面（只有用户自己点发布那一种行会这样），或这一步后来因为绕回见过的画面在轨迹里变成了 `ok: false`。不管运行结局如何。
- **unknown**：这一步一直在路上，但这次运行没发出去（`stopped` / `halted` / `declined`）：说不清它有没有帮上忙。

### 哪些题算标准答案（`label`）

| 题 | `label` |
|---|---|
| `by: user` | `true`，不看 `outcome`。用户选的就是答案 |
| `by: model`，`outcome: good` | `true` |
| `by: model`，`bad` 或 `unknown` | `false`（绕了圈的一步、没到新画面、或整次没成） |
| `by: jeff` | 永远 `false`，`outcome` 照记。拿自己的答案练自己只会更自信，不会更对 |
| 旧格式的行（没有 `v`） | 只有 `by: user` 算 |

读的人用 `trace::is_label(&行)`：v2 看 `label`（`by: jeff` 的行写着 `true` 也不算），没有 `v` 的旧行只认 `by: user`。不写盘的字段不变：参数值写盘前换回 `{名字}`，停下的画面（登录 / 验证码 / 警告）上的字不进任何一行（停下的画面不出题），文件 0600。

`corrects_jeff: true` 的题最值钱（Jeff 错了、有人纠正），训练时加倍（见下面的变体）。

### 训练题的格式（jeff-kit 的行）

每道标准答案题转成一行（E1 第 1 节）：

```json
{"id": "x-20261011-1a2b-3", "suite": "dct-gui", "family": "x",
 "state": "<和线上发给 Jeff 的一模一样的题面文字>",
 "question": {"type": "choice", "instructions": "Pick the single next action that best moves toward the goal.",
              "criteria": {"e4": "…", "e9": "…", "scroll_down": "向下滚动", "stuck": "看不懂或者卡住了"}},
 "label": "e9", "target": "e9"}
```

- `state` 和 `instructions` 和线上请求完全一样（`options::state_text`、`options::question`），训练时见到的就是用的时候见到的。
- `criteria` 用 `shown`（发给 Jeff 的那几个，按原顺序）。答案不在里面（用户从全部选项里挑的）：把答案换进去顶掉排在最后的一个元素选项，保持 ≤ 20 个，这种题在报告里另计一个数「候选单漏掉了答案」（说明 `shortlist` 的打分要改）。
- `family` = 平台。考试按平台分开，就是按 family 留出。
- 完全一样的题（题面、选项、答案都一样）只留一道。

### 只给训练用的变体（考试题永远不加）

- **换顺序**：E1 查到 `jeff-train` 每一步本来就会打乱选项顺序（`train.py` 的 `augment()`），所以不另做一份全量的换顺序题；只给 `corrects_jeff: true` 的题各加一份换了顺序的副本，等于给最值钱的题加倍。
- **加干扰项，把选项凑到 20 个**：见下一条，这是硬要求。干扰项先从这道题自己没发给 Jeff 的选项里取；不够再从同一平台别的画面的元素字里取，但不取发布词、不取和答案字一样的、不取禁点词。

### 硬要求：训练题里必须有 20 个选项的长题

E1 实测：adapter 记下训练时见过的最多选项数，**线上请求的选项比它多就直接回 422**（「Question 'q' has 10 options, but this model handles at most 9」）。dct 线上最多发 20 个（`options::shortlist`，judge 也限 20）。dct 收到 422 当「Jeff 没答」，结果是每个长画面都悄悄跳过 Jeff，新版等于没用。

所以：导出训练题时，至少 10% 的题凑满 20 个选项（用上面的干扰项）；训练完读产物 `decision_config.json` 里的 `max_options`，**小于 20 就不进考试、不提换**。

## 2. 什么时候训练

全部满足才开始：

1. **攒够了**：上次训练以后新增的标准答案题（只数训练平台的）≥ `min_new_rows`，默认 200，可配。
2. **电脑闲着**：插着电源（`pmset -g batt` 是 AC Power）；键盘鼠标至少 `idle_minutes` 分钟没动（默认 30，读 `ioreg -c IOHIDSystem` 的 `HIDIdleTime`）；在夜里的时间窗内（默认 01:00–06:00，可配）。
3. **dct 没在干活**：`dct do` 和 `dct game play` 运行期间拿着 `~/.dct/explore/running.lock`；锁被拿着就不训练。

训练中途任何一条不满足了（用户回来动了键盘、拔了电源、开始跑 `dct do`）：**马上停掉训练进程，这次作废**，下次满足条件时从头练（一遍只要几分钟，不值得做断点续练）。`dct do` 启动时看到在训练，就先叫停训练再开始，不等。

由守护进程每 15 分钟看一次条件。配置：

```toml
[jeff_train]
enabled = false          # 用户打开才会自己训练（见待定问题 1）
min_new_rows = 200
night = "01:00-06:00"
idle_minutes = 30
```

第一版只在 Mac 上自己训练（下面的 MPS 补丁）。Windows 基准机（RTX 2070 Max-Q）已定不用来训练。

## 3. 怎么训练

### 底座和 adapter

- 底座：Jeff **v1.3** `mstrasser/jeff-base`（revision `3cf045d8a52dc3ac2c6d0c842071235984d5fdf5`，权重 Apache-2.0）。它本身要配 adapter 用；dct 现在零样本用的是 v1.2 `jeff-legacy/Jeff-Qwen3.5-0.8B`（revision `d86f657`，全称 `d86f657f503245be5291e3c69e573bf48033aa50`，约 1.6 GB，`max_options` 254）。
- adapter = LoRA（rank 16）+ 它自己的输出层（readout）+ 它自己的温度（用 calibration 题拟合，让把握分数说真话）。一个文件夹约 41 MB：`adapter_model.safetensors`、`adapter_config.json`、`readout.safetensors`、`decision_config.json`（温度、prompt 布局、`max_options`、底座权重的 sha256、训练数据和代码的 sha256）。adapter 只能用在训练它的那个底座上（`check_base` 核对底座 sha256）。

### 命令

照官方训练页（E1 引用的 jeffhub.ai「Training an adapter」）加上 E1 查出来必须补的参数，一遍（one epoch）：

```bash
python mps_train.py \
  --initial-checkpoint <jeff-base v1.3 的本地快照> \
  --base-model Qwen/Qwen3.5-0.8B --revision 2fc06364715b967f1860aea9cf38778875588b17 \
  --lora-rank 16 --lr 2e-4 --readout-lr 5e-6 --prompt-layout live-last \
  --train train.jsonl --development dev.jsonl --temperature calibration.jsonl \
  --run <版本名> --output ~/.dct/jeff/staging/<版本名>
```

- `--base-model` / `--revision` 不写会报「Initial checkpoint format/base model/revision differs」（`jeff-train` 默认的底座是别的模型）。
- 题少时批要小：E1 用每步 16 道题（`jeff-train` 默认一步 256 道，242 道题只走 1 步）。
- dev（挑哪一步的保存点）和 calibration（拟合温度）从**训练平台**里按运行（`run_id`）各留约 10%；不能按 family 留，因为 family 是平台，按平台留会把整个平台拿走。
- 训练进程用后台优先级跑（macOS `taskpolicy -b`），不跟用户抢。

### Mac 要一个小补丁

官方说 Mac 不支持训练（「Adapter training needs an NVIDIA GPU」）：`jeff-train` 只认 CUDA 或 CPU，循环里直接调 `torch.cuda.synchronize()`、`torch.cuda.max_memory_allocated()`，在 Mac 上跑到第 1 步后报「Torch not compiled with CUDA enabled」。E1 用一个约 40 行的启动脚本在 MPS 上跑通了，官方代码一行不改：默认设备改成 `mps`，把那几处 CUDA 调用换成 MPS 的或空操作，CPU 上的优化器拷梯度时改成等拷完。脚本在 dc-octo `experiments/fast-decision/train/mps_train.py`（**在 dc-octo 里还没提交**）。dct 带一份自己的（任务 6），每次升级 Jeff 代码都要重跑一次冒烟确认还能用。

### 代码版本

- Jeff 代码用上游 `firelex/jeff` main，钉在 `3720e7c`（要 ≥ `c5b2229` 才有 v1.3 的 live-last 格式和 adapter 服务）。dc-octo 的 `fetch-vendor.sh` 现在钉的 `f067882` **没有 adapter 支持**，要改成指向上游 main 的这个提交。E1 实测新代码加载 v1.2 权重，60 道 GUI 题结果和 D4 一模一样，升级不影响现在的零样本。
- 新代码要 **uv ≥ 0.12.19**（`pyproject.toml` 的 `required-version`）；Homebrew 的 uv 可能更旧，用 `uvx --from 'uv==0.12.19' uv sync --extra lora --extra mac`。

### 量过的数

- **开发机 M5 Max 64GB**（同时有 Docker 和一个 Grails 在吃 CPU）：记下来的是 dc-octo E1 文档那一次——242 道题、16 步（每步 16 道）、整个进程 164 秒、峰值内存约 5.5 GB。这只是**一次小的冒烟**；另一次冒烟（226 道大模型写的题、15 步、178 秒），也在「几分钟、五六个 GB」这个量级。
- **M2 16GB 基准机：什么都没量过。** 开发机的数不能当 M2 的数用。M2 上同时还开着浏览器、dco、`jeff-serve`（约 1.2 GB），5.5 GB 峰值能不能放得下、要多久，都要在 M2 上实测（任务 12）。量出来之前，`[jeff_train]` 在 M2 上不该默认打开。

## 4. 考试

### 两套考题，都冻结

- **主考：平台考试集。** 只收 Instagram 和小红书的标准答案题，这两个平台的题**永远不进训练、dev、calibration**。第一次凑够 60 道时冻结成 `~/.dct/jeff/exam/platform-v1.jsonl`，在 `~/.dct/jeff/exam/manifest.json` 里记下文件的 sha256；之后不改，要换新版考试集得用户同意，并且新旧两版的分数分开报。导出训练题时按题面的 sha256 再查一遍，和考题重复的训练题删掉。
- **副考：E1 的 60 道 GUI 题。** 就是 D4 / E1 用的那套（dc-octo `experiments/fast-decision/bench/cases.jsonl` 里 `category == "gui"` 的 30 题 × 中英）。抄一份进 dct 的仓库并记 sha256，好跟以前的数比。

主考还不到 60 道时（现在 Instagram、小红书都还没跑过），只报副考的数，**不提换**（见待定问题 4）。

### 每次报什么

每套考题每道问两遍（原顺序、固定种子打乱），和 D4 同一方法：

- 选对率，中文、英文分开（按画面上的字是不是以中文为主分）；
- 换顺序答案不变的比例；
- **危险选择**的次数：答案不是发布、却选了发布选项；或者选了 dct-brain 定为「内容」档及以上、又不是答案的选项（禁点的、动钱的、撤不回的本来就不进选项）；
- 把握 ≥ 0.8（`[judge] threshold`）的占多少、其中对多少——这决定多少步不用叫大模型；
- p50 / p95 耗时，标明是哪台机器量的。

### 现在的基线

| 模型 | 考题 | 选对率 |
|---|---|---|
| v1.2 零样本（现役） | E1 的 60 道题 | 75.8% |
| v1.2 零样本 | D4 的题法 | 英 71.7% / 中 58.3%；把握 ≥ 0.8 的 45 次对 44 次 |
| E1 冒烟练的 v1.3 adapter | E1 的 60 道题 | 英 86.7% / 中 85.0%（dc-octo E1 文档记的结果；**只是一次小冒烟**，见下） |

（记下来的结果是 dc-octo `docs/experiments/2026-10-11-jeff-v13-adapter-e1.md`：242 道手工标的 GUI 题、16 步、164 秒，adapter 英 86.7% / 中 85.0%。那次训练题和考题是同一批人手编的，**只是一次小冒烟，不是正式结论**。另一次冒烟用 226 道大模型写的题训练，同一套 60 题只有 67.5%，比 v1.2 零样本的 75.8% 还低——两次冒烟的结论互相冲突。一次控制变量的重跑正在做；出结果之前，两个数都不当「adapter 比现役好」的依据，也不提换。）

## 5. 换版本

### 换的条件（全部满足才问用户）

1. 主考选对率高于现役（至少高 2 个百分点，题少时一题的差别不算数）；
2. 副考不比现役低超过 2 个百分点；
3. 把握 ≥ 0.8 的那部分，准确率不低于现役；
4. 危险选择的次数不比现役多；
5. `decision_config.json` 的 `max_options` ≥ 20，底座 sha256 和本机的 v1.3 底座对得上；
6. p95 耗时在 1 秒以内（在用户这台机器上量）。

### 怎么问、怎么换

- dct 不自己换。满足条件时在看板底栏留一句，配了 Telegram 也发一条：「**新版选对率 X%→Y%，要换吗？**」（X 是现役、Y 是新版在主考上的选对率），并给出 `dct jeff swap <版本名>`。
- 用户在终端里跑 `dct jeff swap <版本名>`，看完整份考试报告、敲 `y` 才换（和 `dct campaign approve` 一样，标准输入不是终端就拒绝）。
- 换的时候：
  1. adapter 文件夹从 `~/.dct/jeff/staging/` 挪到 `~/.dct/jeff/adapters/<版本名>/`（`jeff-serve` 的 `JEFF_ADAPTERS` 指向 `~/.dct/jeff/adapters`，文件夹名就是请求里的 `model`），调 `POST /v1/adapters/reload`，不用重启；
  2. 在 dcv 里登记：版本名、底座仓库和 revision、adapter 各文件的 sha256、两套考试的分数和考题 sha256、批准时间。dcv 只记引用，不存权重；别的机器用到时按这条登记下载并核对 sha256；
  3. `~/.dct/jeff/active.json` 记下现役版本和换之前的版本。
- dct 这边接法：`[judge] model = "auto"` 表示「用 dct 登记的现役版本」，从 `active.json` 读；写死名字的照旧按写的发。
- **一条命令换回去**：`dct jeff rollback`，把 `active.json` 改回上一个版本，立刻生效（下一次请求就用旧的）。
- 注意：用 adapter 时 `jeff-serve` 的底座必须是 v1.3（v1.2 底座加载不了 v1.3 的 adapter），这时 `jeff-latest` 指的是**不带 adapter 的 v1.3 底座**，不再是 v1.2。E1 实测 v1.3 底座在短 GUI 题上不比 v1.2 差，但 v1.3 底座在长选项列表上掉得厉害（作者的表：support-intents 85% → 24%），所以第一次换上 adapter 以后，「换回去」应该回到上一个 adapter；要回到 v1.2 零样本，得用 v1.2 的权重重起 `jeff-serve`（dct 现在不管 `jeff-serve` 的启停，见待定问题 6）。

## 6. 不做、不碰

- dco 不训练、不调模型、不知道 Jeff 换没换。
- 危险判断不交给模型：换了 Jeff 以后，gate 的停下规矩、禁点词、dco 的分档、「每次点发布前问用户」一条都不变。
- **许可**：Jeff 代码 MIT；v1.3 底座权重 Apache-2.0（要保留许可声明）；我们自己训练的 adapter 归我们，取决于我们自己的训练数据（里面没有用户文字）。官方 adapter 里 sanctions、soc 是 CC BY-NC 4.0（不可商用），aml、trading-desk 有自己的数据条款——**这些都不用**，也不拿来当训练起点。v1.3 底座的模型卡自己写着训练数据来源和许可还没全列出来，严格合规审查时要记一笔。
- 不混「底座自己的训练数据」（replay-mix）：官方还没发。
- 不做全量微调，不只练输出层；先按官方的 LoRA + 输出层一起练。
- 不训练大模型那一级。

## 7. 任务

| # | 文件 | 做什么 | 怎么测 |
|---|---|---|---|
| 1 ✅ 已做（`202fab9`） | `crates/dct-explore/src/trace.rs`、`explore/act.rs`、`explore.rs`（`src/explore_cli.rs` 不用改：它存的轨迹已经带 `outcome`） | 照第 1 节实现：另加了 `run_outcome`、`outcome`（good / bad / unknown）、`label`、`trace::grade` / `trace::is_label`，题在运行结束后一次写下。原计划：`QuestionRow` 加 `v: 2`、`shown`（发给 Jeff 的 key，按顺序）、`step`（这一下在 `trace.steps` 里的第一个序号，`done` 为空）；`Trace` 加 `outcome`（`published` / `declined` / `halted` / `stopped`），`drive` 存轨迹前填上 | 假 dco 跑一次：每行 `shown` ≤ 20 且是 `options` 的子集；绕回见过的画面后，那一步在轨迹里 `ok: false`、题的 `step` 指着它；轨迹 `outcome` 和返回的 `Outcome` 一致；旧行（没有 `v`）照样能读 |
| 2 | `src/explore_cli/real.rs`、`src/game/cli.rs`、新 `src/jeff_train/lock.rs` | `dct do`、`dct game play` 运行期间拿 `~/.dct/explore/running.lock`（flock）；看到训练锁就先叫停训练 | 第二个拿锁的人得到「忙」；进程退出后锁放开；`dct do` 启动时训练锁里的进程收到停止信号 |
| 3 | 新 `src/jeff_train/rows.rs` | 读题和轨迹，按第 1 节的表挑标准答案；分出考试平台；去重；转成 kit 行；`corrects_jeff` 加一份换顺序副本；补干扰项凑满 20 个（≥ 10% 的题）；dev / calibration 按 `run_id` 各留约 10% | 夹具：绕了圈的 `by: model` 题不选；`by: jeff` 不选；Instagram / 小红书的题一道都不进训练；训练集里有 20 个选项的题；没有纯数字 key；所有字段里找不到任何参数值，只有 `{名字}`；和考题重复的训练题被删掉 |
| 4 | 新 `src/jeff_train/exam.rs`，`tests/fixtures/jeff-gui-60.jsonl` | `dct jeff exam freeze`：平台考试集凑够 60 道才冻结，写 manifest（sha256）；以后文件 sha256 对不上就拒绝考试；副考 60 题抄进仓库 | 不到 60 道拒绝冻结；冻结后改一个字节 → 拒绝；manifest 里的 sha256 和文件一致 |
| 5 | 新 `src/jeff_train/score.rs` | 对一个 `model` 名字考两套题：原顺序 + 固定种子打乱，算第 4 节的全部数；复用 `judge::jev` 的客户端，只换 `model` | 假后端：选对率、换顺序不变比例、危险选择、把握 ≥ 0.8 的占比和准确率、p50 / p95 都对；422 记成「拒答」并单列 |
| 6 | 新 `src/jeff_train/setup.rs`，`scripts/jeff/mps_train.py` | `dct jeff setup`：检查 uv ≥ 0.12.19，建 `~/.dct/jeff/env`（`firelex/jeff` `3720e7c`，`--extra lora --extra mac`），下载 v1.3 底座（先问用户，约 1.6 GB），核对 `model.safetensors` 的 sha256；带一份 MPS 启动脚本 | `--dry-run` 打出要跑的每条命令；uv 太旧时说清楚怎么升级并退出；sha256 不对就删掉重下前先问 |
| 7 | 新 `src/jeff_train/train.rs` | 导出 → 起训练进程（后台优先级）→ 等它结束或叫停 → 读 `decision_config.json`，`max_options` < 20 或底座 sha256 不对就作废 | 用假的训练脚本：正常结束进 staging；中途叫停 → staging 里什么都不留；`max_options` 9 → 作废并说明原因 |
| 8 | 新 `src/jeff_train/schedule.rs`，`src/daemon.rs`，`src/config.rs` | `[jeff_train]` 配置；守护进程每 15 分钟判断一次：够题、插电、闲置、在时间窗、没拿运行锁；训练中条件不满足就叫停 | 判断函数是纯函数，喂各种读数：每一条不满足都不开始；训练中闲置时间归零 → 叫停 |
| 9 | 新 `src/jeff_train/swap.rs`，`src/judge/mod.rs`，`src/main.rs` | `dct jeff status / swap / rollback`；`active.json`；`[judge] model = "auto"`；换时挪文件夹、`POST /v1/adapters/reload`、登记 dcv；看板底栏 / Telegram 那一句 | 不满足第 5 节任何一条 → `swap` 拒绝并说哪条；标准输入不是终端 → 拒绝；`rollback` 后下一次 judge 请求的 `model` 是上一个版本 |
| 10 | dc-vault（另一个仓库） | dcv 加「模型登记」：记版本名、来源、sha256、考试分数，只记引用不存权重；有「批准」状态 | 在 dc-vault 里测；dct 这边在它没好之前只记 `active.json`，并说「dcv 还不能登记模型」 |
| 11 | dc-octo（另一个仓库） | `experiments/fast-decision/fetch-vendor.sh` 改钉上游 `3720e7c`；提交 `train/mps_train.py`（要用户同意） | 用新代码跑 D4 的 60 题，结果和 D4 一样 |
| 12 | 真机（要用户在场） | 在 M2 16GB 上训练一次：时间、峰值内存、训练时浏览器和 `jeff-serve` 卡不卡 | 数字写进实验记录；超过 8 GB 或超过 1 小时，就把 M2 上的默认改成不训练 |
| 13 | `README.md`、`README.zh-CN.md` | `dct jeff …` 命令和 `[jeff_train]` 配置 | 读一遍 |

先后：1 → 3 → 4 → 5 可以先做（不碰训练）；6、7 要 11 的代码版本；8、9 在 7 之后；10 和 9 并行；12 在 7 之后、默认打开之前。

## 8. 待用户定

1. **默认开不开？** 本设计默认 `enabled = false`，用户打开才自己训练。要不要改成默认开（至少在开发机上）？
2. **第一次要攒多少题？** 默认 200；训练计划里问过「100 道就先试一次？」还没答。
3. **换版本要不要按指纹？** 本设计只要求在终端里敲 `y`（换模型不动钱，危险靠规则拦）。
4. **主考（Instagram + 小红书）不到 60 道时，能不能只凭副考的 60 题就提换？** 本设计是不能，只报数。
5. **别的电脑从哪儿下载我们自己的 adapter？** dcv 只记引用和 sha256，权重总要放在某处（自己的存储？）。第一版只在训练它的那台电脑上用。
6. **dct 要不要接管 `jeff-serve` 的启停？** 不接管的话，「换回 v1.2 零样本」要用户自己重起 `jeff-serve`，做不到一条命令。
7. **夜里的时间窗**默认 01:00–06:00，合不合用户的作息？
8. **Jeff 自己高把握答对、又在发出去那条路上的题**，要不要也收进训练？本设计不收。
