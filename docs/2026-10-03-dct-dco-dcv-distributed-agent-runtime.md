# 架构决定：dct / dco / dcv 作为统一的分布式 Agent Runtime

日期：2026-10-03  
状态：架构决定与实现方向，不代表所有能力已完成。

## 一句话

> **dct decides. dco acts. dcv remembers what has been confirmed.**

这次决定把三者收敛为一套统一架构，而不是为 desktop、ESP32、robot、drone 再分别发明新的 Agent runtime。

---

## 1. 三个项目的职责

### dct：Brain / Agent Runtime

dct 负责：

- task/session lifecycle；
- planning 与任务分解；
- model routing；
- fast/slow decision；
- durable task state；
- context construction；
- tool/capability selection；
- human steering；
- escalation；
- cost / latency / audit；
- 根据用户规则产生经过授权的执行意图。

dct 不直接控制舵机、GPIO、鼠标、电机或无人机飞控细节。

### DCO：Distributed Capability Protocol / Runtime Family

DCO 不是一个必须经过的中心进程，而是一套统一的 capability / execution 协议与 runtime 家族。dct 可以直接连接任意 DCO node。

DCO node 负责：

- observe；
- execute；
- wait；
- verify；
- stop；
- report；
- permission / ticket checking；
- 设备级安全边界。

任意 DCO node 都不做策略判断，不负责“下一步应该干什么”。

### dcv：Confirmed Knowledge / Procedures / Capability Packages

dcv 负责保存用户确认过的：

- rules；
- procedures；
- recipes；
- app/game/device profiles；
- capability metadata；
- approved model/config references；
- device safety limits；
- reusable expert packages；
- learned but user-approved behaviors。

---

## 2. 为什么这样比单独做 dc-hw / dc-robot runtime 更好

不采用：

```
dco = computer
dc-hw = hardware
dc-robot = robot
dc-drone = drone
```

而采用：

```
                         dct
                brain / orchestrator
                         │
                  DCO protocol
                         │
       ┌─────────────────┼─────────────────┐
       ▼                 ▼                 ▼
 DCO Desktop Node   DCO Edge Node     DCO Edge Node
     Mac/PC            Pi/Linux          ESP32
```

这样上层永远面对 capability，而不是品牌和设备 API。

例如：

```
vision.snapshot
mobility.forward
mobility.rotate
distance.front
audio.play
gpio.set
```

而不是：

```
yahboom_move()
acebott_turn()
dji_takeoff()
esp32_gpio()
```

---

## 3. dct 的模型与决策层

推荐分层：

```
L0 deterministic rules
L1 embeddings / simple classifier
L2 Jeff / Jev-style fast scorer
L3 local LLM / local VLM
L4 cheap cloud model
L5 frontier model
L6 long-horizon multi-agent workflow
```

原则：

- 高频、候选明确的判断尽量走低层；
- 复杂异常和长期规划才升级到 frontier；
- dct 决定是否升级；
- dco 永远不因为模型 confidence 而绕过权限。

---

## 4. dct 面对的是 logical capability，不是 physical device

未来 dct 看到：

```
node: room-robot-01

capabilities:
- vision.snapshot
- mobility.forward
- mobility.rotate
- distance.front
```

而不需要知道它到底是：

- ESP32-S3 小车；
- Raspberry Pi；
- Yahboom；
- ACEBOTT；
- 双足机器人；
- 室内无人机；
- 工业设备。

设备差异由各 DCO node 内部的 adapter 层吸收。

---

## 5. dco-edge

建议把 ESP32 / Linux edge 的实现统一称为：

```
dco-edge
```

第一批 target：

```
dco-edge
├── esp32-s3
├── linux-arm
└── raspberry-pi
```

dct **直接通过 DCO capability protocol 连接这些 node**。不需要先经过 desktop dco。

---

## 6. Physical Agent 的控制分层

以双足机器人为例：

```
dct
目标：去门口
      ↓
dco
动作：walk_forward / turn / stop
      ↓
robot local controller / RL policy
50-200Hz low-level control
```

dct 不负责每 20ms 控制一个关节。

本地 RL / PID / motor controller 不是“更聪明的大脑”，而是高速反射层。

同理：

- 无人机姿态稳定由飞控；
- 电机 PWM 由本地控制器；
- 紧急停车由 edge safety kernel；
- dct 负责高层目标、路径、策略和异常处理。

---

## 7. Fast loop / Slow loop

```
FAST LOOP
sensor / frame
  -> dco observation
  -> local fast decision if needed
  -> dco execution
  -> verify

SLOW LOOP
dct
  -> planning
  -> local/cloud reasoning
  -> new goal / policy / action intent
  -> FAST LOOP
```

不是所有真实世界控制都经过 LLM。

---

## 8. dct 应支持 Virtual Model / Capability Routing

用户不应该选择：

- Jeff；
- Qwen；
- Claude；
- GPT；
- Gemini；
- VLM；

而可以只看到：

```
Auto
Fast
Deep
Local
```

dct 内部根据：

- latency；
- cost；
- privacy；
- modality；
- local hardware；
- provider availability；

映射到实际模型。

---

## 9. Durable Physical Task 需要比 transcript persistence 更多的东西

真实世界任务恢复时必须保存：

- task state；
- observation id；
- last verified action；
- irreversible-action receipt；
- pending approval；
- current device/node；
- device health；
- timeout；
- stale-action rejection data。

崩溃后不能简单重放：

```
unlock_door()
start_motor()
publish()
```

必须先核对是否已经执行成功。

---

## 10. 与 dco / dcv 的契约

dct 向 dco 发送的不是“随便执行这条 API”，而是：

- intent；
- target node；
- capability；
- bounds；
- validity window；
- authorization/ticket；
- expected verification。

dct 从 dcv 读取：

- user rules；
- approved procedures；
- device profiles；
- capability packages；
- safety limits；
- approved model/config references。

---

## 11. 第一阶段不要做太多

先验证：

1. 一个 ESP32-S3 节点；
2. capability discovery；
3. snapshot / sensor / motor / servo / stop；
4. dct -> DCO edge node -> verify 完整闭环；
5. stale command / disconnect / emergency stop；
6. 一套 dcv device profile。

先证明架构，再扩展 robot / drone / industrial devices。

---

## 12. 当前决定

固定以下边界：

> **dct = brain / runtime**  
> **DCO = distributed capability & execution protocol / runtime family**  
> **dcv = confirmed knowledge / procedures / capability packages**

并把 ESP32 看作第一个 physical DCO node target，而不是一个新的独立 Agent 产品。desktop dco 与 dco-edge 是 peer implementations；dct 直接连接任意 DCO node。
