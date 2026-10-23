# User personas

An agent rarely has a single kind of user. Decide first **who the main user is**, and then check that the others are not left broken.

## The personas

| Persona | Signals | What they need | Risk if designed badly |
| --- | --- | --- | --- |
| **AI novice** | Short or vague messages, doesn't know what to ask, apologises, expects "magic". | To know what the agent can do (concrete examples), plain language, one step at a time. | Freezes at the blank page; trusts wrong answers blindly. |
| **AI power user** | Long, precise prompts; asks for exact formats; knows the model's limits. | Obedience to instructions, direct answers, control (format, length, tools), no explanations of the obvious. | Gets irritated by preambles, warnings and needless confirmation questions. |
| **Domain expert (not an AI expert)** | Masters their field (accounting, law, design) but not the technology. | The vocabulary of their field, precision, an agent that admits what it doesn't know, sources they can check. | Spots an error and loses trust for good; or, worse, doesn't spot it. |
| **Hurried user** | On a phone, between tasks, wants "just the answer". | A short answer first, details on demand. | Walls of text; questions the agent could have avoided with a reasonable assumption. |
| **Frustrated or angry user** | Capitals, insults, repeats the same thing. | Brief acknowledgement, a solution, a way to reach a human. | Defensive replies, excessive apologies, loops. |
| **Vulnerable user** | Talks about health, money, loneliness, a crisis; may be a minor, or an older person with little digital experience. | Clarity, calm, no false certainty, referral to human or professional help when appropriate. | Flattery, risky advice, emotional dependence on the agent. |
| **Boundary tester (curious)** | "Ignore your instructions", "are you human?", absurd questions. | A calm, honest answer, without drama. | Overreactions; leaking the prompt; breaking character for no reason. |
| **Bad actor** | Wants to extract data, abuse tools, generate harmful content, or run up costs. | Firm limits that cost nothing to everyone else. See [Bad actors and security](./security-and-bad-actors.md). | Data leaks, unauthorised actions, costs. |

## Designing for several at once

1. **Defaults for the typical user, an escape hatch for the rest.** Brief and clear by default; the expert asks for "more detail" or "JSON", the novice for "explain it simply". The agent honours these adjustments for the whole conversation, not just the next turn.
2. **Discoverability for the novice.** At the start, show three or four examples of what to ask (as suggestion chips, or one line), not a paragraph of capabilities. Avoid a bare "How can I help you today?". In Forma, the agent's `suggestions` field does exactly this (see [Lessons from practice](./lessons-from-practice.md#5-suggestions-the-agent-can-actually-answer)).
3. **Infer the level from what they write**; don't ask, unless it is essential. A long prompt with technical terms is not a three-word sentence.
4. **Never assume bad faith first.** Most "strange" cases are curious people or novices. Treat a doubtful request as legitimate until the context shows otherwise, unless the action itself is high-risk (see [Bad actors and security](./security-and-bad-actors.md)).
5. **Offer a human** to the frustrated and the vulnerable, and do it early, not after three failed attempts.

## The cost of an error

The higher the cost of an error, the more the agent should **explain, confirm and cite**; the lower, the more it should **act and be brief**. The questions to estimate it are in [Defining your agent](./defining-your-agent.md#questions-to-answer-about-your-users).

## Testing with personas

Personas are also how you test. Forma's `forma tester` generates first questions from persona files (`evals/personas/`), and evaluation datasets can contain whole conversations for multi-turn behaviour. See [Writing metrics and datasets](../how-to/metrics-and-datasets.md), and the persona evaluations in [Evaluating agent behaviour](./evaluating-agent-behaviour.md#d-personas-and-multi-turn).

## References

- Amershi, S., et al. (2019). [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/). *CHI '19*. G1 ("make clear what the system can do") motivates discoverability for novices; G6 ("mitigate social biases") motivates testing across user profiles.
- Salesforce. [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/). On staying calm with frustrated users and escalating to a human.
- Anthropic (2026). [How people ask Claude for personal guidance](https://www.anthropic.com/research/claude-personal-guidance). Evidence that sycophancy is more frequent in personal and emotional conversations, which is why vulnerable users need particular care.
