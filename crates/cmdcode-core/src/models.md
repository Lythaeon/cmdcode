<!-- Generated from the Command Code docs: https://commandcode.ai/docs -->

# Command Code Models

The model catalog — every id `/model`, `--model`, `model:effort` shorthand, and agent-file `model:` pinning accept, with context windows, reasoning efforts, and advertised per-1M-token rates. Ids are EXACT — never invent or guess a model id. A BYO-provider id (from a configured custom provider) also works and is passed through as-is.

## Open Source

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `deepseek/deepseek-v4-pro` | DeepSeek V4 Pro (latest) | 1M | high, max | $0.66/$1.98 · cache $0.022 | Go and above | hybrid-attention long-context reasoning |
| `deepseek/deepseek-v4-flash` | DeepSeek V4 Flash (latest) | 1M | high, max | $0.22/$0.66 · cache $0.007 | Go and above | fast hybrid-attention reasoning |
| `deepseek/deepseek-v4-flash-vision-exp` | DeepSeek V4 Flash Vision (exp) | 1M | high, max | $0.22/$0.66 · cache $0.01 | Go and above | fast hybrid-attention reasoning with vision |
| `moonshotai/Kimi-K3` | Kimi K3 | 1M | — | $3/$15 · cache $0.3 | Go and above | long-horizon coding & knowledge work with 1M context |
| `moonshotai/Kimi-K2.7-Code` | Kimi K2.7 Code | 256K | — | $0.95/$4 · cache $0.19 | Go and above | improved long-horizon coding with vision |
| `moonshotai/Kimi-K2.7-Code-Highspeed` | Kimi K2.7 Code HighSpeed | 262K | — | $1.9/$8 · cache $0.38 | Go and above | high-speed long-horizon coding with vision |
| `moonshotai/Kimi-K2.6` | Kimi K2.6 | 256K | — | $0.95/$4 · cache $0.16 | Go and above | long-horizon coding with vision |
| `moonshotai/Kimi-K2.5` | Kimi K2.5 | 256K | — | $0.6/$3 · cache $0.1 | Go and above | multimodal frontend coding |
| `zai-org/GLM-5.3` | GLM-5.3 | 1M | low, high, max | $1.4/$4.4 · cache $0.26 | Go and above | frontier coding with emergent cyber capabilities |
| `zai-org/GLM-5.2` | GLM-5.2 | 1M | high, max | $1.4/$4.4 · cache $0.26 | Go and above | powerful coding with 1M context and long-horizon tasks |
| `zai-org/GLM-5.2-Fast` | GLM-5.2 Fast | 1M | — | $3/$10.25 · cache $0.5 | Go and above | high-throughput GLM-5.2 with 1M context |
| `zai-org/GLM-5.1` | GLM-5.1 | — | — | $1.4/$4.4 · cache $0.26 | Go and above | long-horizon autonomous coding agent |
| `zai-org/GLM-5` | GLM-5 | 200K | — | $1/$3.2 · cache $0.2 | Go and above | multi-mode thinking & long-range planning |
| `MiniMaxAI/MiniMax-M3` | MiniMax M3 | 1M | — | $0.3/$1.2 · cache $0.06 | Go and above | frontier coding, agents & native multimodality |
| `MiniMaxAI/MiniMax-M2.7` | MiniMax M2.7 | — | — | $0.3/$1.2 · cache $0.06 | Go and above | end-to-end software engineering agent |
| `MiniMaxAI/MiniMax-M2.5` | MiniMax M2.5 | 200K | — | $0.3/$1.2 · cache $0.03 | Go and above | cross-platform full-stack agentic dev |
| `xiaomi/mimo-v2.5-pro` | MiMo V2.5 Pro | 1M | — | $0.435/$0.87 · cache $0.0036 | Go and above | high-capability long-context agentic coding |
| `xiaomi/mimo-v2.5` | MiMo V2.5 | 1M | — | $0.14/$0.28 · cache $0.0028 | Go and above | efficient long-context agentic coding |
| `Qwen/Qwen3.8-Max` | Qwen 3.8 Max | 1M | low, medium, xhigh | $2/$6 · cache $0.25 (write $2.5) | Go and above | autonomous long-horizon coding & professional work |
| `Qwen/Qwen3.8-27B` | Qwen 3.8 27B | 262K | low, medium, xhigh | $0.4/$3 · cache $0.04 | Go and above | compact vision-language coding & agentic work |
| `Qwen/Qwen3.7-Max` | Qwen 3.7 Max | 1M | — | $2.5/$7.5 · cache $0.5 (write $3.13) | Go and above | frontier coding & long-horizon agent execution |
| `Qwen/Qwen3.7-Plus` | Qwen 3.7 Plus | 1M | — | $0.4/$1.6 · cache $0.08 (write $0.5) | Go and above | agentic coding & reasoning at lower cost |
| `Qwen/Qwen3.7-Flash` | Qwen 3.7 Flash | 1M | — | $0.03/$0.13 · cache $0.006 (write $0.038) | Go and above | fast low-cost agentic coding & reasoning |
| `Qwen/Qwen3.6-Max-Preview` | Qwen 3.6 Max Preview | — | — | $1.3/$7.8 · cache $0.26 (write $1.63) | Go and above | vibe coding & efficient agent execution |
| `Qwen/Qwen3.6-Plus` | Qwen 3.6 Plus | — | — | $0.5/$3 · cache $0.1 | Go and above | agentic coding & reasoning |
| `stepfun/Step-3.7-Flash` | Step 3.7 Flash | 256K | — | $0.2/$1.15 · cache $0.04 | Go and above | multimodal sparse-MoE reasoning |
| `stepfun/Step-3.5-Flash` | Step 3.5 Flash | 1M | — | $0.1/$0.3 · cache $0.02 | Go and above | fast sparse-MoE agentic reasoning |
| `tencent/hy3-paid` | Tencent Hy3 | 262K | — | $0.14/$0.58 · cache $0.035 | Go and above | sparse-MoE reasoning & agentic tool use |
| `nvidia/nemotron-3-ultra-550b-a55b` | Nemotron 3 Ultra | 1M | — | $0.6/$2.4 · cache $0.12 | Go and above | open reasoning model for long-horizon autonomous agents |
| `thinkingmachines/inkling` | Inkling | 256K | — | $1/$4.05 · cache $0.17 | Go and above | multimodal MoE reasoning |
| `thinkingmachines/inkling-small` | Inkling Small | 1M | — | $0.5/$1.2 · cache $0.1 | Go and above | lightweight MoE reasoning at lower cost and latency |
| `poolside/laguna-s-2.1-free` | Laguna S 2.1 | 256K | — | $0/$0 · cache $0 | Go and above | open-weight agentic coding and long-horizon work |
## Stealth

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `stealth/ox-alpha` | Ox Alpha | 1.05M | low, high, max | $0/$0 · cache $0 | Go and above | long-horizon coding, agentic work & visual context |
## Anthropic

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `claude-sonnet-5` | Claude Sonnet 5 | 1M | low, medium, high, xhigh, max | $2/$10 · cache $0.2 (write $2.5) | Pro and above | best combo of speed & intelligence (recommended) |
| `claude-sonnet-4-6` | Claude Sonnet 4.6 | 1M | low, medium, high, xhigh, max | $3/$15 · cache $0.3 (write $3.75) | Pro and above | prev Sonnet, still fast & capable |
| `claude-fable-5` | Claude Fable 5 | 1M | low, medium, high, xhigh, max | $10/$50 · cache $1 (write $12.5) | Max | most capable for demanding reasoning & long-horizon agents |
| `claude-opus-5` | Claude Opus 5 | 1M | low, medium, high, xhigh, max | $5/$25 · cache $0.5 (write $6.25) | Max | most intelligent Opus for agents and coding |
| `claude-opus-4-8` | Claude Opus 4.8 | 1M | low, medium, high, xhigh, max | $5/$25 · cache $0.5 (write $6.25) | Max | prev flagship, still strong for agents and coding |
| `claude-opus-4-7` | Claude Opus 4.7 | 1M | low, medium, high, xhigh, max | $5/$25 · cache $0.5 (write $6.25) | Max | older Opus, still strong for agents and coding |
| `claude-haiku-4-5-20251001` | Claude Haiku 4.5 | 200K | — | $1/$5 · cache $0.1 (write $1.25) | Pro and above | fastest & most compact, great for quick tasks |
## OpenAI

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `gpt-5.6-sol` | GPT-5.6 Sol | 1.05M | low, medium, high, xhigh, max | $5/$30 · cache $0.5 (write $6.25) | GOAT and above | frontier model for complex professional work |
| `gpt-5.6-terra` | GPT-5.6 Terra | 1.05M | low, medium, high, xhigh, max | $2/$12 · cache $0.2 (write $2.5) | Pro and above | balances intelligence and cost |
| `gpt-5.6-luna` | GPT-5.6 Luna | 1.05M | low, medium, high, xhigh, max | $0.2/$1.2 · cache $0.02 (write $0.25) | Go and above | optimized for cost-sensitive workloads |
| `gpt-5.5` | GPT-5.5 | 400K | low, medium, high, xhigh | $5/$30 · cache $0.5 (write $0) | Pro and above | latest frontier model for general complex work |
| `gpt-5.4` | GPT-5.4 | 400K | low, medium, high, xhigh | $2.5/$15 · cache $0.25 (write $0) | Pro and above | frontier model for general complex work |
| `gpt-5.3-codex` | GPT-5.3 Codex | 400K | low, medium, high, xhigh | $2/$8 · cache $0.5 (write $0) | Pro and above | frontier coding model |
| `gpt-5.4-mini` | GPT-5.4 Mini | 400K | low, medium, high | $0.75/$4.5 · cache $0.075 (write $0) | Pro and above | fast, cost-effective model for everyday tasks |
## Google

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `google/gemini-3.7-flash` | Gemini 3.7 Flash | 1.05M | low, medium, high | $0.75/$3.75 · cache $0.075 (write $0.04167) | GOAT and above | higher-quality coding & agentic workflows, fewer tokens |
| `google/gemini-3.6-flash` | Gemini 3.6 Flash | 1M | low, medium, high | $1.5/$7.5 · cache $0.15 | Pro and above | previous Gemini Flash, still fast & capable |
| `google/gemini-3.5-flash` | Gemini 3.5 Flash | 1M | low, medium, high | $1.5/$9 · cache $0.15 | Pro and above | Pro-level coding proficiency, parallel agentic execution |
| `google/gemini-3.5-flash-lite` | Gemini 3.5 Flash Lite | 1M | low, medium, high | $0.3/$2.5 · cache $0.03 | Pro and above | upgraded agentic capabilities, ideal for subagents |
| `google/gemini-3.1-flash-lite` | Gemini 3.1 Flash Lite | 1M | low, medium, high | $0.25/$1.5 · cache $0.03 | Pro and above | high-volume workhorse model with implicit caching |
## Sakana

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `sakana/fugu-ultra` | Fugu Ultra | 1M | high, xhigh | $5/$30 · cache $0.5 | Max | multi-agent orchestration across frontier models |
## Meta

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `meta/muse-spark-1.1` | Muse Spark 1.1 | 1.05M | — | $1.25/$4.25 · cache $0.15 | Pro and above | agentic performance, tool use, and computer use |
| `meta/muse-spark-1.2` | Muse Spark 1.2 | 1.05M | — | $1.25/$4.25 · cache $0.15 | GOAT and above | coding-optimized for agentic workflows and large codebases |
| `meta/muse-spark-1.2-contributor` | Muse Spark 1.2 Contributor | 1.05M | — | $0.1/$0.2 · cache $0.002 | Go and above | Muse Spark 1.2 at ~95% off |
## xAI

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out · cache read | Min plan | Best for |
|---|---|---|---|---|---|---|
| `xai/grok-4.5` | Grok 4.5 | 500K | low, medium, high | $2/$6 · cache $0.5 | Go and above | smartest model for coding, agentic tasks, knowledge work |
| `xai/grok-4.6` | Grok 4.6 | 500K | low, medium, high, xhigh | $2/$6 · cache $0.5 | GOAT and above | frontier performance on coding, knowledge work, and STEM |

Rates are the advertised price list resolved from the billing source of truth (promos are already baked in): input / output / cache-read per 1M tokens, plus cache-write where the provider bills it. Min plan is the cheapest public plan that serves the model, ordered Go < GOAT < Pro < Max (see plans.md). Models without an effort column entry decide their own reasoning depth. Full pricing detail: https://commandcode.ai/docs/resources/pricing-limits
