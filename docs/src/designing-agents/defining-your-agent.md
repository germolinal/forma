# Defining your agent

Before writing a single line of prompt, decide what the agent is for. It sounds obvious, and it is the step most often skipped. Without it, nobody (not you, not a colleague, not an AI assistant helping you build the agent) can say whether the agent works, because "works" has not been defined.

This matters even more when you build agents with the help of an AI coding assistant, as Forma's `forma init` encourages (it ships an `agent-dev-guidelines.md` for that purpose). An assistant that lacks these answers will invent them, quietly. So will a person.

## Write it down: the agent's README

Keep the definition in the agent's `README.md`, next to its YAML, where anyone changing the agent will find it. It should answer:

| Section | Question it answers | Example |
| --- | --- | --- |
| **Goal** | What does the user achieve thanks to the agent? One or two sentences. | "Small business owners understand their monthly tax obligations and file them on time." |
| **Users** | Who is the main user, and who else may reach it? | Main: owners with no accounting training. Secondary: their bookkeepers. |
| **Cost of an error** | What happens if the agent is wrong? | A missed deadline means a fine: high. |
| **Language and tone** | Which language and register, three or four adjectives, how much small talk. | Spanish, informal "tú", calm and precise; level 2 small talk (see [Small talk and scope](./small-talk-and-scope.md)). |
| **Scope** | What it does, what it refers elsewhere (and to whom), what it declines, what it refuses. | Does: explain obligations. Refers: complex cases to an accountant. Declines: unrelated homework. Refuses: helping evade taxes. |
| **Tools** | What each tool does, and which source comes first. | Knowledge base first; web search only for what it doesn't cover. |
| **Personalisation** | What it remembers about each user, if anything. | Business type, tax regime, preferred level of detail. |

If any of these is missing or ambiguous, resolve it with whoever owns the agent before touching the prompt.

> **Note**: In Forma, the `README.md` that `forma init` creates (the one about "Dory" and Ollama) is an example project's README, not the definition of *your* agent. Replace it.

## The cost of an error drives everything

The single most useful question in that table is the cost of an error. It decides how the agent should behave in almost every other chapter:

- **High cost** (money, legal, health, irreversible actions): the agent should **explain, confirm and cite**. It states its uncertainty, points to official sources, and asks before acting.
- **Low cost** (a draft, a suggestion, something easy to undo): the agent should **act and be brief**. It makes a reasonable assumption and says so, instead of interviewing the user.

See [User personas](./user-personas.md) for the questions that help you estimate it, and [Questions at the end](./closing-questions.md) for how it changes when to ask.

## Questions to answer about your users

- Who uses the agent, and how often (once, or daily)?
- Do they know what a language model is? Do they know it can be wrong?
- Which device and channel (mobile, desktop, WhatsApp, voice)?
- What happens if the agent is wrong: annoyance, lost money, legal or health risk?
- Could minors, people in a delicate situation, or sensitive data be involved?

## From the definition to everything else

The definition is not paperwork; each section feeds a later decision:

- **Users and cost of an error** → [User personas](./user-personas.md), and which [evaluations](./evaluating-agent-behaviour.md) block a release.
- **Language and tone** → [Tone and personality](./tone.md), and the tone section of the [prompt template](./checklist-and-prompt-template.md).
- **Scope** → [Small talk and scope](./small-talk-and-scope.md), and the datasets for "should answer" versus "should decline".
- **Tools** → tool descriptions, [role-based access](../how-to/role-based-tools.md) and [human approval](../how-to/human-in-the-loop.md) for risky actions (see [Bad actors and security](./security-and-bad-actors.md)).
- **Personalisation** → what the agent stores, and how users can see and delete it ([Errors, uncertainty and trust](./trust-errors-uncertainty.md), guideline G17).

## References

- Amershi, S., et al. (2019). [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/). *CHI '19*. Guidelines G1 and G2 ("make clear what the system can do, and how well") start from exactly this definition.
- [Lessons from practice](./lessons-from-practice.md), section 1, for how the README is used when developing agents with an AI assistant.
