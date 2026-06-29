Here is a summary of `added_tokens` from [DeepSeek API Docs](https://api-docs.deepseek.com/zh-cn/quick_start/token_usage):

| Token                                                  | special  | normalized | Purpose                                                        |
| ------------------------------------------------------ | -------- | ---------- | -------------------------------------------------------------- |
| `<think>`                                      | false    | **true**   | **Reasoning chain container** (Chain-of-Thought). Reasoning models like DeepSeek-R1 output internal thinking within this tag before generating the final answer, typically collapsed in display. |
| `<\|fim▁hole\|>` / `<\|fim▁begin\|>` / `<\|fim▁end\|>` | false    | **true**   | **Fill-In-the-Middle (code completion)**. `begin` and `end` mark prefix/suffix code blocks, `hole` marks the middle position the model needs to fill. |
| `<\|User\|>` / `<\|Assistant\|>`                       | false    | **true**   | **Role anchors**. Replace traditional `User:` / `Assistant:` text prefixes as more robust structured separators, preventing role confusion attacks (prompt injection). |
| `<\|EOT\|>`                                              | **true** | **true**   | **End of Turn**. Marks the end of the current turn, one of the signals for the model to stop generating. |
| `<\|tool▁calls▁begin\|>` / `<\|tool▁calls▁end\|>`      | false    | **true**   | **Tool call list container**. Wraps all tools to be called in this turn. |
| `<\|tool▁call▁begin\|>` / `<\|tool▁call▁end\|>`        | false    | **true**   | **Single tool call container**. Typically contains JSON-formatted function name and arguments. |
| `<\|tool▁outputs▁begin\|>` / `<\|tool▁outputs▁end\|>`  | false    | **true**   | **Tool return results list container**. |
| `<\|tool▁output▁begin\|>` / `<\|tool▁output▁end\|>`    | false    | **true**   | **Single tool return result container**. |
| `<\|tool▁sep\|>`                                       | false    | **true**   | **Tool separator**. Used to separate multiple tool calls or return results within the same turn. |
| `<\|begin▁of▁sentence\|>` / `<\|end▁of▁sentence\|>`    | **true** | false      | **Sequence-level boundary markers** (BOS/EOS). Mark the physical start and end of the entire input/output sequence. |
| `<\|▁pad▁\|>`                                          | **true** | false      | **Padding token** (PAD). Used for sequence length alignment during batch inference, model does not generate attention for it. |

Then here is the actual conversation testing on the DeepSeek web interface:

![image-20260429105126264](assets/图1.png)

From the above image, we can see that after being filtered by the web backend, the tokens that are actually usable are only `<think>` `` `` `<|User|>` `<|Assistant|>`, so:

- I plan to use `< | System | >` for a compromise system prompt injection;
- ~~Will use instruction rules to constrain the model into a special mode for tool calls, using `< | Tool | >` to represent tool call results;~~
- Meanwhile as shown below, `<think>` when left unclosed can guide the model to think, enabling more powerful rule injection (reminder);

![image-20260429110516352](assets/图2.png)

## Subsequent Experimental Findings

Through actual testing, the native tag `<|tool▁calls▁begin|>` when used as the primary tag caused severe model confusion. Suspect the backend has special processing or filtering for `<|...|>` fullwidth format.

Tried a compromise using `<|tool▁calls▁begin|>` / `<|tool▁calls▁end|>` as tool call tags:

- Using ASCII `|` instead of fullwidth `|` avoids triggering backend filtering while retaining native-tag-like structural feel
- **Effect was surprisingly good** — model recognition and compliance significantly improved, hallucinations greatly reduced
- Possible reason: tokenizer has existing token patterns for `<|...|>` format, model has better compliance tendency for this "structural template"

**Current strategy: experiment-driven, incremental maintenance.**

- Primary tag: `<|tool▁calls▁begin|>` / `<|tool▁calls▁end|>`
- Fallback list is empty by default, append individually to `extra_starts` / `extra_ends` when model hallucination variants are discovered
- `<|tool▁calls▁begin|>` format rarely causes model hallucinations, saving significant fallback maintenance effort
