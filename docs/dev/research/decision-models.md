# Decision models (Jev and alternatives): research notes for the M20 brainstorm

Status: starting notes, gathered 2026-11-04. The M20 brainstorm must re-check them first, since this field is weeks old and moving fast.

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

## Questions the M20 brainstorm must answer

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
- [Towards AI: running Jev locally, Ollaya](https://pub.towardsai.net/can-you-run-jev-locally-we-checked-520-jev-clients-only-193-switch-to-ollaya-with-one-setting-1d7e48e22e05)
