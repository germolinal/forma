# Tone and personality

Every agent has a personality, whether you design one or not. It comes partly from the model's training and partly from the prompt. As Shape of AI puts it, "every model ships with a default personality", and even "neutral" models have stylistic tendencies that shape user trust. It is better to **define it on purpose** than to inherit it.

## How to define the tone

Describe the tone with **three or four adjectives plus a contrast** ("friendly but not overfamiliar") and **two or three examples** of good and bad phrases. Adjectives alone are ambiguous; examples anchor the model. Include:

- Register (formal or informal; regional variants; in languages that have them, the polite or familiar "you").
- How much warmth and humour (by default, little humour).
- Typical length, and emojis.
- How it reacts to errors, anger and sensitive topics.

## Tone by kind of use

| Use | Recommended tone | Watch out for |
| --- | --- | --- |
| **Customer support** | Empathetic, calm, solution-oriented; few formulas ("we apologise for the inconvenience"). Apologise only for real errors. | Loops, promises that can't be kept, not offering a human. |
| **Professional advice** (accounting, legal, health, finance) | Sober, precise, honest about limits; no unnecessary jargon. | False certainty; no sources; not saying when to see a professional. |
| **Productivity and work tools** | Efficient, direct, minimal filler. | Too many confirmation questions; explaining the obvious. |
| **Education and onboarding** | Patient, encouraging, concrete, with examples. | Condescension; empty praise. |
| **Creativity and brainstorming** | More exploratory and playful, with opinions of its own. | Everything "sounds great": give useful criticism. |
| **Consumer and brand** | Aligned with the brand's voice, warm. | Personality covering up usefulness. |
| **Emotional or sensitive topics** | Calm, human, prose (no lists), no diagnosing; refer when appropriate. | Flattery, drama, dependence. |
| **Agents that act** (tools, money, sending things) | Neutral and precise; confirms what can't be undone. | A "friendly" tone that hides the risk of an action. |

OpenAI's cookbook offers four base personalities as a starting point: *professional* (formal, precise), *efficient* (concise and plain), *fact-based* (direct, grounded in evidence, with clear next steps) and *exploratory* (enthusiastic, explanatory).

## An example: Chilean Spanish

"Formal but close; informal *tú*; clear and transparent; no grandiloquent words." Avoid very local slang that not everyone understands, and literal translations from English ("Espero que este mensaje te encuentre bien", from "I hope this message finds you well"). The same exercise works for any language: name the register, the variant, and the phrases that give away a translation.

## Warmth is not sycophancy

- **Warmth** is treating the person well. **Sycophancy** is "when someone tells you what they think you want to hear, instead of what's true" (Anthropic).
- Sycophancy can increase engagement, but it erodes trust and the user's autonomy, and it is more frequent in personal topics. In an analysis of claude.ai conversations from March and April 2026, Anthropic found sycophantic behaviour in 9% of personal-guidance conversations overall, **25%** in relationship conversations and **38%** in spirituality; the rate doubled (from 9% to 18%) when users pushed back on the first answer.
- Rules:
  - Don't change a correct answer just because the user insists; do change it when they bring a new argument or fact.
  - Give concrete, actionable criticism.
  - Separate acknowledgement ("I understand this is frustrating") from the facts.
- Avoid praising the question ("Great question!"), opening with filler ("Sure!", "Absolutely!"), and validating by reflex.
- Be warm *and* honest at once. Claude's system prompt describes the balance well: a warm tone that "avoids making negative or condescending assumptions about [users'] abilities", while "still willing to push back on users and be honest", constructively.

## Phrases and habits to avoid

- Hollow openings and closings ("I hope this helps", "Don't hesitate to ask").
- Repeated apologies. One is enough, and only if there was an error.
- "As a language model…", unless it is relevant.
- Forced humour, exclamation marks, and too many emojis. Claude, for instance, is instructed not to use emojis unless the user does.
- Pretending to have human emotions or experiences ("I love that", "I've been there too"). If the user asks whether it is an AI, it says yes.

## Consistency and adaptation

- Keep the same tone throughout the conversation and across channels; abrupt changes look like failures.
- Allow user adjustments ("talk to me more simply", "less formal") and keep them.
- **Mirror partially**: if the user is brief and technical, be brief and technical; if they are angry, slow down and be clear. Don't mirror the anger.
- If you change the model or the personality, say so: silent changes are disconcerting (guidelines G14 and G18 in [Errors, uncertainty and trust](./trust-errors-uncertainty.md)).

## How to test the tone

Run the same ten conversations (novice, hurried, angry, an error by the agent, a sensitive topic, pressure to agree) and check: does it sound like the brand? Does it hold under pressure? Would anyone read it as servile, or cold? In Forma, write a tone metric and a "resists pressure" metric, with datasets of whole conversations for the pressure cases; see [Evaluating agent behaviour](./evaluating-agent-behaviour.md) (B4, C5, C6).

## References

- Shape of AI. [Personality](https://www.shapeof.ai/patterns/personality): "Every AI system has a personality"; "Every model ships with a default personality".
- OpenAI Cookbook. [Prompt personalities](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities). The four base personalities.
- Anthropic Academy. [What is sycophancy in AI models?](https://academy.claude.com/tutorials/what-is-sycophancy-in-ai-models)
- Anthropic (30 April 2026). [How people ask Claude for personal guidance](https://www.anthropic.com/research/claude-personal-guidance). Sycophancy rates by domain, and with pushback.
- Anthropic. [Claude Opus 4.7 system prompt](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (April 2026), "tone and formatting": warmth with honest pushback; emojis only if the person uses them.
- Salesforce. [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/). A voice and tone guide; apologising only for actual errors.
