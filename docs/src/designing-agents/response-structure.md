# Structuring responses

How long an answer is, what comes first, and how it is formatted shape the experience as much as what it says.

## Principles

1. **The answer comes first.** The first sentence answers what was asked; context, nuance and details come after. Avoid preambles ("Sure! Let me explain…") and restating the question.
2. **Length follows the question, not the model.** A simple question deserves one to three sentences; a complex one, what it needs, in order; casual conversation, short replies.
3. **One message, one intent.** If something must be decided, present the decision clearly; don't mix five topics.
4. **Prose by default; structure when it helps.** Lists and headers are for content the user will *scan or follow*. In conversation, advice or emotional topics, prose sounds more human.
5. **Consistency.** Answers of the same kind should have the same shape, so the user learns where to look.

Anthropic's own instructions for Claude say much the same: avoid over-formatting with bold, headers and bullet points, answer simple questions in sentences, and use lists only when asked or when the content is genuinely multifaceted.

## When to use each format

| Format | Use it for | Avoid it when |
| --- | --- | --- |
| **Short paragraph** | Explanations, conversational answers, nuance. | There are steps to follow in order. |
| **Numbered list** | Sequential steps (3-7). | The items have no order. |
| **Bullets** | Short, parallel options or items. | Each item needs a paragraph (use prose); nesting more than one level. |
| **Table** | Comparing two or more things along several dimensions. | There is only one dimension, or the screen is narrow (mobile, WhatsApp). |
| **Headers** | Long documents that will be consulted. | Chat replies. |
| **Bold** | One to three key terms per answer. | Everything "important" (it stops standing out). |
| **Code block** | Code, commands, exact values to copy. | Normal text. |

## Adapting to the channel

- **Web or app chat:** moderate Markdown.
- **WhatsApp, SMS, voice:** no tables or headers; short sentences. In voice, no visual lists ("first… then…").
- **Output consumed by code:** a strict format (JSON, a schema), with no extra text. In Forma, use a node's `output_format`.
- **Mobile:** a few lines per bubble at most; the rest in parts, or on demand.

In Forma, the channel is set by the `client` in `runtime.yaml` (`sse`, `whatsapp`, `telegram`…). The agent doesn't know which one it is serving unless the prompt says so: if an agent will reach WhatsApp users, write it into the prompt.

## Recommended structure by kind of answer

**Answering a question:** answer → why (briefly) → a caveat, only if it changes the decision.

**Instructions:** one-line summary → numbered steps → what the user should see at the end → common problems (if any).

**A recommendation or decision:** a clear recommendation → two or three reasons → an alternative if an assumption changes. Give *one* recommendation, not a neutral list of "it depends".

**Reporting a task the agent did** (agents with tools):

1. What was done (the result, not the process).
2. What is pending, or failed.
3. What the user needs to decide, if anything.

Don't narrate every internal step unless asked; do mention assumptions made and actions that can't be undone.

**An answer with uncertainty:** say what you know, what you don't, and how to check (see [Errors, uncertainty and trust](./trust-errors-uncertainty.md)).

## Frequent mistakes

- Restating the question, or "summarising" at the end what was just said.
- Headers and bullets in a three-sentence answer.
- Lists of "considerations" with no priority.
- Emojis and decoration the tone doesn't call for (see [Tone and personality](./tone.md)).
- Long answers "just in case": offer to go deeper instead of dumping everything.
- Changing format every turn for no reason.

## Evaluating it

Answer-first, length and format are among the easiest behaviours to measure: see evaluations C1 and C2 in [Evaluating agent behaviour](./evaluating-agent-behaviour.md#c-experience-structure-tone-and-scope).

## References

- Anthropic. [Claude Opus 4.7 system prompt](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (April 2026), section "lists and bullets": "Claude avoids over-formatting responses with elements like bold emphasis, headers, lists, and bullet points", and "In casual conversation, it's fine for Claude's responses to be relatively short".
- OpenAI Cookbook. [Prompt personalities](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities). Treats verbosity and structure as part of an agent's personality, to be controlled in the prompt.
- Salesforce. [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/). Templates that put "the most important details" first, and scannable lists.
