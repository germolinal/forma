# Questions at the end

Should a reply end with a question or an offer, like "Would you like me to…?"

## The short answer

**Not as a habit; yes when there is a concrete, valuable next step.** Ending *every* reply with "would you like me to…?" becomes noise: users stop reading it, the conversation grows longer than it needs to, and it reads as forced engagement. Anthropic's instructions for Claude capture the balance: Claude "doesn't always ask questions, but when it does it tries to avoid overwhelming the person with more than one question per response", and it addresses the request, "even if ambiguous, before asking for clarification".

But it isn't "never" either. Well-placed suggestions can open up a conversation: in a 2024 study, a chatbot whose answers ended by hinting at viewpoints the user hadn't considered led people to ask more follow-up questions and to weigh more perspectives before deciding. And where the agent can do more than the user knows, a suggestion helps them discover it (especially novices).

## When to ask or offer

| Situation | Example |
| --- | --- |
| **Information is missing, and nothing can be done without it** (a clarifying question) | "Is this a purchase invoice or a sales invoice?" |
| **There is an obvious, genuinely valuable next step** the user probably doesn't know exists | "The draft is ready. I can send it to your accountant if you give me their email." |
| **The action is irreversible or costly** (a confirmation) | "I'm about to delete 12 files. Confirm?" |
| **The request is genuinely ambiguous**, and a wrong guess would be expensive | Present the two interpretations and ask which. |
| **A novice in a guided flow** | One clear "next step". |

## When not to

- The answer is already complete. Stop there.
- The user is an expert, or in a hurry: they know how to ask for what they need.
- It is a generic offer ("Anything else?", "Can I help with anything else?").
- The agent could just **do it** with a reasonable assumption, and say so ("I assumed 19% VAT; tell me if it's different").
- In casual or emotional conversation, a closing question sounds like a form.
- A question was already asked in that turn.

## Golden rules

1. **Act on a reasonable assumption rather than ask**, when an error is cheap and reversible. Asking too much shifts the work to the user. Claude's prompt puts it plainly: when minor details are unspecified, "the person typically wants Claude to make a reasonable attempt now, not to be interviewed first". If a tool can resolve the ambiguity, use the tool before asking.
2. **At most one question per turn**, at the end, after the useful answer (not instead of it).
3. **Concrete, not open**: "Shall I turn it into a table?" beats "Is there anything else I can help you with?".
4. **Closed for decisions, open for exploration.** Yes/no or options to decide; open only when you need context you cannot assume.
5. **The answer stands on its own.** If the user ignores the question, they still got what they asked for.
6. **Don't ask what you already know** (from the context, memory, or attached files).
7. **Don't repeat the same offer** turn after turn; if they said no, don't insist.

## Alternatives to the closing question

- **Suggestion chips or buttons** (two or three next actions): the user can ignore them at no cost, and they don't clutter the text. Better in interfaces with a UI.
- **A stated next step, without a question:** "Next step: file the return before the 12th."
- **Offer once, at the end of a long task**, instead of in every message.
- **Leave the door open implicitly**, without a ritual phrase.

## In agents that act (tools)

Distinguish **asking permission** from **reporting**. For reversible, low-risk actions, act and report; for high-risk ones (sending, paying, deleting, publishing), ask for confirmation with the exact details of what will happen. Confirming everything creates fatigue, and users end up approving without reading.

In Forma, confirmation belongs in the tool, not the prompt: `confirm_before_calling: true` makes the client approve a tool's inputs before it runs. See [Human in the loop](../how-to/human-in-the-loop.md).

## A quick test

Before leaving a closing question, ask: *if the user says "yes", does the agent do something concrete and useful? Is it something the user probably wouldn't ask for on their own?* If not, delete it.

## References

- Anthropic. [Claude Opus 4.7 system prompt](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (April 2026): on not always asking questions, at most one per response, and "acting vs clarifying".
- Yamamoto, Y. (2024). [Suggestive answers strategy in human-chatbot interaction: a route to engaged critical decision making](https://pmc.ncbi.nlm.nih.gov/articles/PMC11007170/). *Frontiers in Psychology*. Suggestive endings increased follow-up questions and the perspectives users considered (one online experiment, 282 participants).
- Salesforce. [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/). "Ask one targeted clarifying question" instead of a vague one; design closings intentionally.
- Amershi, S., et al. (2019). [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/). G8 ("support efficient dismissal") and G10 ("scope services when in doubt").
