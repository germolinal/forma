# Designing AI agents

Getting an agent to answer is easy. Getting it to answer *well* is not: in the right length, with the right tone, without flattering the user, without inventing facts, without obeying instructions hidden in a web page, and without asking "anything else?" at the end of every message. This part of the documentation is about that second, harder problem: the **experience** of talking to an agent, and how to design it on purpose.

It is written for anyone building agents, with Forma or with anything else. Most of it is about people and language models, not about Forma. Where Forma has a feature that implements an idea (a field, a tool, a command), the chapter says so.

> **Last reviewed:** October 2026. Models change quickly, and every new version brings its own stylistic habits (more verbosity, more bullet points, more flattery). If something here depends on a particular model, check it against yours.

## How these chapters are organised

| Chapter | What it covers |
| --- | --- |
| [Defining your agent](./defining-your-agent.md) | What to decide *before* writing a prompt: goal, users, cost of an error, scope, tone. |
| [User personas](./user-personas.md) | Who talks to the agent (novices, power users, domain experts, the hurried, the frustrated, the vulnerable, bad actors) and how to design for several at once. |
| [Structuring responses](./response-structure.md) | Length, order (the answer first), when to use lists, tables and headers, adapting to the channel, reporting work done. |
| [Tone and personality](./tone.md) | Tone by kind of use, warmth versus sycophancy, phrases to avoid, consistency. |
| [Questions at the end](./closing-questions.md) | Should every reply end with "would you like me to…?" When yes, when no, and the alternatives. |
| [Small talk and scope](./small-talk-and-scope.md) | How transactional an agent should be; greetings, chit-chat, off-topic and out-of-scope requests. |
| [Errors, uncertainty and trust](./trust-errors-uncertainty.md) | Microsoft's 18 Guidelines for Human-AI Interaction applied to agents; communicating uncertainty, owning errors, escalating. |
| [Bad actors and security](./security-and-bad-actors.md) | Prompt injection, tool abuse, data extraction; defences that live in the system, and refusing well. |
| [Evaluating agent behaviour](./evaluating-agent-behaviour.md) | A battery of evaluations for security, trust and experience, how to score them, and when to run them. |
| [Checklist and prompt template](./checklist-and-prompt-template.md) | A review checklist and a reusable "how you converse" section for system prompts. |
| [Lessons from practice](./lessons-from-practice.md) | What the author of Forma learned building agents with it: conventions, Forma-specific settings, and mistakes worth avoiding. |

The first ten chapters summarise published guidance and research, with references at the end of each. The last one is different: it is experience, not literature.

## Where to start

- **"I'm designing a new agent."** Start with [Defining your agent](./defining-your-agent.md) and [User personas](./user-personas.md), then [Tone](./tone.md) and [Small talk and scope](./small-talk-and-scope.md), and finish with the [Checklist](./checklist-and-prompt-template.md).
- **"The answers are too long, or full of bullet points."** [Structuring responses](./response-structure.md).
- **"Should it always end with a question?"** [Questions at the end](./closing-questions.md).
- **"It agrees with everything I say."** [Tone and personality](./tone.md#warmth-is-not-sycophancy).
- **"Users go off topic."** [Small talk and scope](./small-talk-and-scope.md).
- **"It got something wrong, or doesn't know."** [Errors, uncertainty and trust](./trust-errors-uncertainty.md).
- **"Someone is trying to manipulate it."** [Bad actors and security](./security-and-bad-actors.md).
- **"How do I know it does all of this?"** [Evaluating agent behaviour](./evaluating-agent-behaviour.md), together with Forma's [evaluations](../documentation/evals.md).

## Three rules that apply everywhere

1. **There is no single right answer.** Almost everything depends on *who* uses the agent, *what for*, and *through which channel*. Decide those three things before deciding tone, length or how much small talk to allow.
2. **What the prompt says is not what the model does.** Test with real (or simulated) conversations for each kind of user, and adjust. This is what [evaluations](../documentation/evals.md) are for.
3. **Design the default for the most common case, and let users change it.** "Be shorter", "explain it simply", "give me JSON": the agent should honour these for the rest of the conversation.

## Main sources

These chapters lean on a handful of sources, cited in detail where they are used:

- Amershi et al. (2019), [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/), CHI 2019; and Microsoft's [HAX Toolkit](https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/).
- Shape of AI, [Personality](https://www.shapeof.ai/patterns/personality).
- OpenAI Cookbook, [Prompt personalities](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities).
- Anthropic, [Claude's system prompts](https://platform.claude.com/docs/en/release-notes/system-prompts), and [How people ask Claude for personal guidance](https://www.anthropic.com/research/claude-personal-guidance) (2026).
- OWASP, [LLM01: Prompt Injection](https://genai.owasp.org/llmrisk/llm01-prompt-injection/), the [LLM Prompt Injection Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html), and the [Top 10 for Agentic Applications (2026)](https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026).
- Salesforce, [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/).
