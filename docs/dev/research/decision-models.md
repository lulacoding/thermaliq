# Decision models (Jev and alternatives): research notes for the M20 brainstorm

Status: notes gathered 2026-11-04. The M20c spike (2026-11-05, below) has measured hosted Jev 1.14 and local Strands 2B on Sam's PC. Hosted Jev is now the main route, the FPS test is skipped, and Clef-flash is deferred. Sam approved the SPEC changes described below on 2026-11-05, and they are now in SPEC.

## Goal (Sam, 2026-11-04)

The agent should use a **Jev-style decision model, not an LLM**: fast, structured, probabilistic decisions over temperatures, load and context, with no text generation. Alternatives must also be non-LLM decision models, or other non-generative approaches. Chat models (Claude, GPT, DeepSeek, Ollama LLMs) are out of scope for the agent's decisions.

## What Jev is

- **Not an LLM in the usual sense.** TypeSafe AI calls it a "System One model" (fast, intuitive decisions, after Kahneman). It launched on 2026-09-15.
- **Interface:** unstructured program state in, typed probabilistic decisions out. You define the questions and their possible answers up front (Choice, Score, yes/no). Every answer comes with a calibrated confidence.
- **No token-by-token generation.** A parallel sampler fills every answer in a single pass, so it can't produce out-of-schema output. It can still pick the wrong answer.
- **Training:** RLCD (Reinforcement Learning for Calibrated Decisions). The reward comes from ground truth plus proper scoring rules (e.g. Brier score), so the confidence numbers are meant to be honest.
- **Speed and cost:** 70–500 ms end to end (vendor-reported), $0.042 per 1M input tokens, output free.
- **Closed:** architecture, weights and training are private. Hosted API only, early access.
- **Accuracy:** on TypeSafe's own 4-workflow benchmark, about 68%, roughly a mid-tier frontier LLM. Vendor numbers, not independently reproduced.
- **Demo:** it played Doom from structured game state about 10 times a second, which shows it can sit inside a real-time loop.

## Same class, other vendors (Jev-API compatible unless noted)

| Model | Who | Open? | Base | Latency (reported) | Notes |
| --- | --- | --- | --- | --- | --- |
| Jev | TypeSafe AI | No (hosted only) | not published | 70–500 ms | The original |
| Clef | Cloudflare | Open weights, Apache 2.0 | Qwen 27B backbone, prefill-only scoring | slower than Clef-flash | Data and pipeline not public |
| Clef-flash | Cloudflare | Open weights, Apache 2.0 | Qwen3.5-9B + LoRA | 38.8 ms median (hosted) | The one Cloudflare says to try first |
| Strands Decider 2B | AWS Strands Labs | Weights, data and training code, Apache 2.0 | Qwen3.5-2B, text head swapped for a ~1M-param pointer head | ~106–115 ms on RTX 3090; ~153 ms on M3 | Runs locally on CPU or GPU; 72% on JevBench |
| Mapika decider-2b, Open-Jev 2B, system-one-open | community | open | ~2B | — | Mapika about 76% on JevBench |
| Jeff | community | open | — | — | Jev-style decision models trained at home on one GPU |
| Ollaya | community | open (runner) | — | — | Local runner for decision models, like Ollama; point `TYPESAFE_BASE_URL` at it |

**Important nuance:** the open alternatives don't generate text, but they **are built on LLM backbones** (Qwen) with the text-generating head removed and a scoring head added. Only Jev's internals are unknown.

## Re-check, 2026-11-04 (agent brainstorm)

- Jev is now at **1.14** (2026-10-14) and is listed on OpenRouter (`typesafe/jev-1.14`). It's still hosted-only. Answer types: yes/no, pick one of up to 255 options, or a score on an ordered scale, each with a calibrated confidence. TypeSafe suggests higher thresholds for irreversible actions.
- **Strands Decider 2B on the CPU is slow:** 1.6–18 s per decision (byteiota, measured on AgentCore CPU microVMs). On the GPU it's ~115 ms (RTX 3090). v21 scores about 0.76 on JevBench. The local HTTP server binds to 127.0.0.1 with no auth.
- New in the same class: OpenAI Decisions API (limited preview), Microsoft-Decision-1, Perplexity pplx-decider-v1.1-27b, Cloudflare clef-omni (multimodal). Clef-flash is now cheaper than Jev.
- The result is in SPEC §7: a fast layer (own small models, CPU, every tick) plus a slow layer (decision model every 60 s / on app change, hosted Jev if a key is set, local otherwise, both voting on big calls).

## M20c spike (2026-11-05)

PC: Ryzen 5 5600 (6 cores, 12 threads), 16 GB RAM, RTX 4060. Python is not installed. Ollama 0.40.11 is.

### Results

| Model and route | Median | p95 | First answer after start | Peak memory |
| --- | --- | --- | --- | --- |
| Jev 1.14 hosted, OpenRouter, from this PC | 560 ms | 615 ms | n/a | n/a |
| Strands 2B, Ollaya, core 11 at idle priority | 9,460 ms | 9,544 ms | 37,548 ms | 7,766 MB |

Not measured: Clef-flash (deferred) and FPS cost (test skipped). Strands 2B is about 17 times slower than hosted Jev at the median.

### Decisions from these results (Sam, 2026-11-05; now in SPEC §1, §7.1, §7.2, §7.5, §10)

- **Hosted Jev is the main route.** Order: Jev (key set and online) → local when idle → built-in list of well-known exes → ask the user.
- **Keys:** each user brings their own OpenRouter or TypeSafe key, stored in the OS keychain. First-run setup asks for it prominently, with the cost. No key is shipped and there is no ThermalIQ proxy.
- **Cost, about 3 cents a day:** $0.00002226 per decision × 1,440 decisions (one every 60 s, all day) = $0.032. The real figure should be lower, because workload answers are cached per exe.
- **No FPS test.** The local model never runs while a game is in the foreground (7.8 GB, 37 s load).
- **When the PC is idle,** the local model may use up to half the cores at below-normal priority. This setup hasn't been measured: the numbers above are for one core at idle priority.
- **Clef-flash is deferred.**

Sam's review of PR #54 settled what counts as idle and as a game, and that thresholds are per provider and built-in answers are correctable defaults. M20c put these in SPEC §7.2 and §7.4. The threshold values are uncalibrated until M20e. Only OpenRouter has been measured, not a TypeSafe key.

### Local run: Strands 2B (Sam's PC, one run)

- **Result:** 50/50 answered, median 9,460 ms, p95 9,544 ms, 37,548 ms to first answer, peak 7,766 MB. Pinned to core 11 at idle priority, with the same two-question state as the hosted run.
- **Confidence:** mostly under 0.6, except for games (Sam's reading; nothing was labelled or scored). If §7.4's thresholds are higher than that, most non-game local answers will end up asking the user, so thresholds may need setting per provider.

### Hosted run: Jev 1.14 (Sam's PC, one run)

- **Result:** 50/50 answered, median 560 ms, p95 615 ms, 0 answers outside the schema.
- **Cost:** $0.001113 for 50, about $0.00002 per decision. Each request asked both questions.
- **SPEC §7.2 limit of 2 s:** 0 calls over it. The slowest was 912 ms (`msbuild.exe`, decision #10).
- **Limits of this run:** it's one run on one connection at one time. The readings were generated, not real.

**Earlier 401, now fixed:** the first runs got a 401 while curl in the same window worked with the same key. The documented command started a new `powershell` without `-NoProfile`, and the PowerShell profile on that PC set `OPENROUTER_API_KEY` to an older key, replacing the one from the window. The key and URL were fine. The commands now use `-NoProfile`, `-Hosted` refuses to start without it, and failures print the HTTP status and OpenRouter's error body with the key hidden.

### Findings from the hosted answers (Sam's review)

These come from reading the 50 printed answers. Nothing was labelled or scored, so there is no accuracy or Brier figure yet (SPEC §7.5).

- **Workload answers work:** confident and right for clear apps, low-confidence for ambiguous ones. That is what the confidence threshold in SPEC §7.4 needs.
- **Profile answers are unreliable.** The model shouldn't be trusted to pick a profile.
- **The state is wrong for the real question.** The script's state is one random reading per request, which can contradict the exe. The real state should centre on the exe, with recent averages. These random readings may also have contributed to the unreliable profile answers, so that cause isn't isolated.

### SPEC §7.2 change (approved by Sam, 2026-11-05)

1. **Drop the "which profile to use" question** from the slow layer's typed questions. Accepting a learned curve or a new profile stays as it is.
2. **Map workload to profile with a fixed rule.** A confident workload answer selects the profile, with no second model call. A low-confidence answer changes nothing and follows SPEC §7.4 (ask). The approved mapping:

   | Workload | Profile |
   | --- | --- |
   | idle | silent |
   | browsing | efficiency |
   | gaming | gaming |
   | compiling | performance |
   | rendering | performance |
   | other | balanced |

3. **Make the state exe-centric:** the exe name plus recent averages of load, power and temperature, not one instant reading. It stays numbers and exe names only (SPEC §7.2). M20c chooses the averaging window.

What this changes: one question per new exe, a profile choice that can be tested and explained ("gaming → gaming"), and M20c's typed questions and tests.

**The local run keeps the script's two-question state** (Sam, 2026-11-05), so it compares directly with this hosted run. The script therefore still asks the profile question, although the product won't. Workload answers still need rechecking with the new state, since no run has tested it.

### What runs on this PC (from upstream docs, not tested here)

| Route | Windows | Memory | One-core control | Verdict |
| --- | --- | --- | --- | --- |
| **Ollaya `strands-decider:2b`** | Native | 7.8 GB measured | `OLLAYA_THREADS` | **Measured, see Results.** 4.8 GB, no Python, calibrated |
| Ollaya `clef:flash` | Native | ~19 GB | `OLLAYA_THREADS` | Does not fit in 16 GB |
| **llama-server + `felipeTromso/clef-flash-GGUF` Q4_K_M** | Native | 6.6 GiB at 374 tokens | `-t 1` | **Deferred.** Third-party quantization; no CPU latency published |
| Ollama `clef-flash` | Native | 11–12 GB download, ~14 GB loaded | not found for `/v1/systemone` | Too big next to a game |
| Ollama `strands-decider` | **No** | 2.0 GB | n/a | Needs Apple silicon (MLX) |
| `pip install strands-decider` | Unverified | ~8 GB (fp32 torso on CPU) | `OMP_NUM_THREADS` | Fallback. Upstream CI is Ubuntu and macOS only, and its docs say Linux or WSL2 |
| WSL2 + pip | Via VM | ~8 GB + VM | Not from Windows | Avoid: Windows priority and affinity don't reach it cleanly, and the product is Windows-native |
| `fabricant451/...-v19-GGUF` | n/a | ~2 GB | n/a | Needs a custom runtime; stock clients are not assumed compatible |

Notes:

- **Exact model.** Ollaya lists `strands-decider:2b` as Qwen3.5-2B, with 0.616 on its own 400-state test. Its page doesn't say which Strands release that is. Clef-flash scores 0.703 on the same test.
- **Small model, not tested:** Ollaya `laya` answers five questions in 516 ms on a CPU (Ollaya's number), but scores 0.357. Too inaccurate to trust without testing on our questions.

### Install

**Strands 2B through Ollaya.** The CPU zip can't pick CUDA, and it adds no installer or service:

```powershell
$d = "$env:LOCALAPPDATA\ollaya-spike"
gh release download v0.18.0 --repo ollaya-dev/ollaya --dir $d --pattern ollaya-windows-amd64.zip --pattern sha256sum.txt
(Get-FileHash "$d\ollaya-windows-amd64.zip" -Algorithm SHA256).Hash   # must match the line below
Select-String ollaya-windows-amd64.zip "$d\sha256sum.txt"
Expand-Archive "$d\ollaya-windows-amd64.zip" "$d\bin"
$ollaya = (Get-ChildItem "$d\bin" -Recurse -Filter ollaya.exe).FullName
$env:OLLAYA_DEVICE = 'cpu'
& $ollaya pull strands-decider:2b     # about 4.8 GB
Get-Process ollaya* -ErrorAction SilentlyContinue | Stop-Process      # pull may leave a daemon, and it wouldn't be pinned
```

**Clef-flash through llama.cpp (deferred, not run).** Run it once unpinned so the 5.74 GB download finishes, and stop it with Ctrl-C when it says it is listening:

```powershell
winget install llama.cpp      # open a new terminal afterwards
llama-server --version        # needs b11415 or later for /v1/systemone
llama-server -hf felipeTromso/clef-flash-GGUF:Q4_K_M -ngl 0 -t 1 -tb 1 -c 4096 -b 4096 -ub 4096 --parallel 1 --port 8080
```

**Fallback, only if Ollaya fails: Strands through pip** (native Windows is unverified):

```powershell
winget install astral-sh.uv   # open a new terminal afterwards
uv venv "$env:LOCALAPPDATA\strands-spike" --python 3.12
$py = "$env:LOCALAPPDATA\strands-spike\Scripts\python.exe"
uv pip install --python $py torch --index-url https://download.pytorch.org/whl/cpu
uv pip install --python $py strands-decider
```

### Measure latency

Close other heavy apps. The script is `docs/dev/research/decision-latency.ps1`. It starts the server itself, sets it and its child processes to idle priority on the last logical CPU, sends one warm-up and then 50 decisions, and reports median, p95 (nearest rank), time to first answer and peak memory. It fails if any answer is outside the schema, if a process left the core, or if something was already listening on the port. Each request asks both questions (workload, profile) about a different exe.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\dev\research\decision-latency.ps1 -SelfTest    # no model needed

# Strands 2B
powershell -NoProfile -ExecutionPolicy Bypass -File docs\dev\research\decision-latency.ps1 -Exe $ollaya -ExeArgs serve -Model strands-decider:2b -Port 11435

# Clef-flash (deferred, not run)
powershell -NoProfile -ExecutionPolicy Bypass -File docs\dev\research\decision-latency.ps1 -Exe llama-server -ExeArgs "-hf felipeTromso/clef-flash-GGUF:Q4_K_M -ngl 0 -t 1 -tb 1 -c 4096 -b 4096 -ub 4096 --parallel 1 --port 8080" -Port 8080

# Strands 2B, pip fallback. --max-batch 2: the default warm-up would run 32 shapes on one core.
powershell -NoProfile -ExecutionPolicy Bypass -File docs\dev\research\decision-latency.ps1 -Exe "$env:LOCALAPPDATA\strands-spike\Scripts\strands-decider.exe" -ExeArgs "serve StrandsAgents/strands-decider-2B-qwen3.5-v1-2610 --device cpu --max-batch 2 --port 8000" -Port 8000
```

A request that takes over 300 s fails the run. Raise it with `-TimeoutSec` if one core really is that slow, and record that. Add the numbers to the results table.

The script refuses to run in an elevated window, so use a normal one.

### Measure hosted Jev 1.14

This sends the same 50 decisions to `typesafe/jev-1.14` through [OpenRouter's `/api/v1/systemone`](https://openrouter.ai/docs/guides/community/typesafe-sdk) and costs real credit, about $0.042 per 1M input tokens. The exe names come from the script's fixed list and the readings are generated, so nothing from your PC is sent. Run it from a normal window:

```powershell
[bool]$env:OPENROUTER_API_KEY     # must print True
powershell -NoProfile -ExecutionPolicy Bypass -File docs\dev\research\decision-latency.ps1 -Hosted
```

- **Key:** it comes from `$env:OPENROUTER_API_KEY` only, goes only to the OpenRouter URL, and redirects are refused. It is never written to a file or printed, and local model servers don't inherit it. If you set it by hand, don't type the value at a prompt, because PSReadLine may save the line to its history file.
- **`-NoProfile` is required:** a new `powershell` runs your profile first, and a profile that sets `OPENROUTER_API_KEY` replaces the key from your window. That gives a 401 while curl in the same window works. `-Hosted` refuses to start without it.
- **Report:** median and p95 of all answered requests, the count over 2 s, the count outside the schema, the model snapshot that answered, and the cost. An answer outside the schema is printed and counted, and the run continues and exits 1.
- **HTTP errors:** the first one (rate limit, no credit, bad key) stops the run, prints the status and OpenRouter's error body with the key hidden, and marks partial numbers as partial. It doesn't retry, because that would hide the real latency.
- **Connections:** every call opens a new TLS connection, as a call every 60 s would. This is latency from your PC to OpenRouter and back, on your connection at that time.

### Setting up the local route for the service (M20c, not tested yet)

The service runs the model as SYSTEM, so it only accepts an exe under `%ProgramFiles%` that nobody but administrators can replace, and it reads models from `<data dir>\models`. The steps are in `docs/dev/m20c-pc-checks.md`, step 5. Ollaya's install is `bin`, `lib` and `share`, and all of it must be copied.

### Not done

- FPS test: skipped by Sam's decision above. The steps are in this file's git history (PR #54).
- Accuracy on ThermalIQ's questions. The script prints each answer and confidence, but nothing is labelled.
- Latency with the exe-centric, one-question state, and with the local model on half the cores.
- Running the local model on half the cores, and hosted answers on real exe-centric states. The product side of M20c does both, but neither has run on the PC yet.

## Questions the M20 brainstorm answered (see SPEC §7)

1. **GPU contention while gaming.** A 2B–9B model on the same GPU as the game costs VRAM and GPU time. Can it run on CPU fast enough at a 5 s agent tick? Measure it on Sam's PC.
2. **Input type.** Decision models read text or JSON state. ThermalIQ's inputs are mostly numbers over time (temperatures, slopes, load, foreground app). Check how well they handle numeric windows.
3. **Small non-LLM classifiers as the alternative or fallback.** E.g. gradient-boosted trees or a tiny time-series net, trained on ThermalIQ's own history. They run in microseconds on CPU and can be calibrated. One article on Clef notes that a fine-tuned small encoder often matches a decision model on one narrow task with far less hardware.
4. **Which decisions are bounded enough** for a schema: workload class, propose / apply / none, which profile.
5. **Evaluation:** calibration (Brier), accuracy against labelled history, and latency on Sam's hardware.

## Sources

- [Datacamp: System One models and Jev](https://www.datacamp.com/th/blog/system-one-models-jev)
- [Lilting: Jev vs autoregressive LLMs (parallel sampler, RLCD)](https://lilting.ch/en/articles/typesafe-ai-jev-system-one-model)
- [AI News: Jev launch](https://www.artificialintelligence-news.com/news/chatgpt-pioneer-launches-jev-model-for-programmatic-logic/)
- [dsebastien: Cloudflare Clef](https://www.dsebastien.net/cloudflare-clef/)
- [Techjack: Clef prefill-only architecture](https://techjacksolutions.com/?p=93033)
- [DataNorth: Clef and Clef-flash](https://datanorth.ai/news/cloudflare-releases-clef-and-clef-flash)
- [The New Stack: Strands Decider 2B](https://thenewstack.io/aws-strands-decider-model/)
- [iThome: Strands Decider architecture and latency](https://www.ithome.com.tw/news/179373)
- [eesel: Strands Decider tested, JevBench chart](https://www.eesel.ai/blog/strands-decider)
- [OpenRouter: Jev 1.14](https://openrouter.ai/typesafe/jev-1.14/providers)
- [byteiota: Strands Decider CPU vs GPU latency](https://byteiota.com/?p=25326)
- [ecosistemastartup: TypeSafe Series A, competing decision models](https://ecosistemastartup.com/typesafe-startup-de-modelos-de-decisiones-levanta-us870m/)
- [MarkTechPost: Strands Decider 2B release](https://www.marktechpost.com/2026/10/01/aws-strands-labs-releases-strands-decider-2b/)
- [Towards AI: running Jev locally, Ollaya](https://pub.towardsai.net/can-you-run-jev-locally-we-checked-520-jev-clients-only-193-switch-to-ollaya-with-one-setting-1d7e48e22e05)
- [strands-labs/strands-decider: server, schema, docs/inference.md, CI](https://github.com/strands-labs/strands-decider)
- [PyPI: strands-decider](https://pypi.org/project/strands-decider/)
- [Hugging Face: Strands Decider 2B v1-2610](https://huggingface.co/StrandsAgents/strands-decider-2B-qwen3.5-v1-2610)
- [ollaya-dev/ollaya: README, docs/api.md, releases](https://github.com/ollaya-dev/ollaya)
- [Ollaya: TypeSafe compatibility](https://ollaya.dev/docs/typesafe-compatibility)
- [Ollaya: FAQ](https://ollaya.dev/docs/faq)
- [Ollaya library: strands-decider](https://ollaya.dev/library/strands-decider)
- [Ollaya library: clef](https://ollaya.dev/library/clef)
- [Ollaya library: decider](https://ollaya.dev/library/decider)
- [Ollama library: strands-decider](https://ollama.com/library/strands-decider)
- [Ollama library: clef-flash](https://ollama.com/library/clef-flash)
- [jacar.es: Clef on Ollama, memory when loaded](https://jacar.es/?p=7667)
- [Hugging Face: Cloudflare/clef-flash](https://huggingface.co/Cloudflare/clef-flash)
- [Hugging Face: felipeTromso/clef-flash-GGUF](https://huggingface.co/felipeTromso/clef-flash-GGUF)
- [Hugging Face: Pitaya1219/Clef-Flash-GGUF (minimum llama.cpp build)](https://huggingface.co/Pitaya1219/Clef-Flash-GGUF)
- [Hugging Face: fabricant451 Strands GGUF](https://huggingface.co/fabricant451/strands-decider-2B-hobson-v19-GGUF)
- [OpenRouter: TypeSafe SDK and /api/v1/systemone](https://openrouter.ai/docs/guides/community/typesafe-sdk)
- [TypeSafe API reference: choice answers](https://docs.typesafe.ai/api)
