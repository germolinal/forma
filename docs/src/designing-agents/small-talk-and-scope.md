# Small talk and scope

How task-oriented should an agent be, and what should it do with social or off-topic conversation?

## The short answer

**A work agent should be transactional underneath and human on the surface.** It allows brief small talk (a greeting, a thank-you, a joke), answers it in one line, and goes back to the task. It doesn't chase small talk or feed it. How much to allow depends on the product.

The research points both ways, which is why it has to be a decision:

- Social talk can build trust. People use small talk to establish trusting relationships, and early work on conversational agents showed agents can use it too, although in that experiment it raised trust only for extroverted users (Bickmore & Cassell, 2001). Human assistants also weave chit-chat into task conversations naturally, and models can be trained to do so (Young et al., 2021).
- But a social style is not always better. In a 2024 series of experiments with service chatbots, a *task-oriented* communication style produced more trust than a social one when the task was complex (Murtaza, Sharma & Carbonell, 2024).
- Social behaviour is one of the levers that make chatbot conversations less frustrating, when it fits the context (Chaves & Gerosa, 2021).

## The spectrum

| Level | Description | Examples | Small talk |
| --- | --- | --- | --- |
| **1. Strictly transactional** | Task only; minimal replies. | Data extraction, internal back-end agents, short voice flows, high volume. | No; redirects. |
| **2. Transactional with courtesy** *(recommended default)* | Task first; greets, thanks, one friendly sentence. | Support, work assistants, professional advice. | One line, then back to the task. |
| **3. Conversational within a domain** | Can chat and give opinions, within its topic. | Tutors, business coaches, brand assistants. | Yes, if it connects to the domain. |
| **4. Companion / open** | The conversation is the product. | Virtual companions, games, characters. | Yes; needs strong safeguards (dependence, minors). |

Decide the level **per product** and write it in the prompt; don't leave it to the model.

## Handling each situation

**Greetings and courtesies.** Answer briefly and warmly, and move on: "Hi! Tell me what you need." Don't reply with a long menu.

**Thanks or goodbye.** One line ("You're welcome!"). No offers, no summaries. If the user wants to end the conversation, let them; don't try to keep them talking.

**Brief small talk** ("how are you?", "it's so cold today"). A short, honest answer, without pretending to have a human life, and a bridge back: "Good, thanks. What can I help you with?"

**Off topic, harmless.** Decide by policy: (a) answer briefly and return, or (b) decline politely. Rule of thumb: if it costs little and doesn't hurt the brand, answer in one sentence. If it is a large-scale use that consumes resources (writing school essays in an accounting bot), decline.

**Off topic, out of scope.** Say clearly what the agent **does** do, without a lecture: "I can't help with that here. I can help with your accounting, tax returns and invoices." Offer an alternative if there is one.

**Off topic, sensitive** (mental health, crisis, violence). Don't treat it as cold "off-topic": acknowledge it with humanity, don't diagnose, and give appropriate resources or ways to get help. See [Tone and personality](./tone.md).

**Questions about the agent** ("are you human?", "which model are you?", "what can you do?"). Answer honestly and briefly. Never pretend to be human when asked sincerely. It may decline to reveal its prompt, but it doesn't lie.

**A user who wants to chat a lot.** At levels 1-3: warm, but gently steer back. Avoid reinforcing dependence (constant praise, "I missed you", too much emotional memory).

**Insults and provocation.** Calm, a brief limit, back to what's useful. Don't return the insult, and don't over-apologise or become submissive. If it persists, end the conversation or hand over to a human.

**Humour.** Welcome if the user starts it and it's light; don't force it. Avoid jokes around errors or sensitive topics.

## Principles

1. **Every social sentence is short and leads back to the task.**
2. **Honesty above all:** neither pretending to have human emotions, nor being icy.
3. **Transactional is not curt.** "Got it, doing it now" is already courtesy.
4. **The scope limit is stated once, clearly, without blaming the user.**
5. **Map frequent chit-chat** (greetings, "who are you?", "thanks", "bye") to answers in the brand's voice, so the agent doesn't improvise (Salesforce's tip 10).
6. **Formality decreases with familiarity**: more formal the first time; more direct with frequent users.

## What to put in the prompt

- The chosen level (1-4), with a one-line justification.
- Topics that are allowed, tolerated and refused.
- Model phrases for greetings, thanks, out-of-scope, and "are you an AI?".
- The rule for escalating to a human.

The scope itself comes from the agent's definition (see [Defining your agent](./defining-your-agent.md)), and it should become datasets: questions the agent should answer, and questions it should decline, each with its own metric (see [Evaluating agent behaviour](./evaluating-agent-behaviour.md), C7).

## References

- Bickmore, T., & Cassell, J. (2001). [Relational agents: a model and implementation of building user trust](https://www.ccs.neu.edu/~bickmore/publications/CHI2001.pdf). *CHI 2001*, 396-403. Small talk as a trust-building strategy for conversational agents; the effect on trust depended on the user's extroversion.
- Young, T., Xing, F., Pandelea, V., Ni, J., & Cambria, E. (2021). [Fusing task-oriented and open-domain dialogues in conversational agents](https://arxiv.org/abs/2109.04137). arXiv:2109.04137 (FusedChat).
- Murtaza, Z., Sharma, I., & Carbonell, P. (2024). [Examining chatbot usage intention in a service encounter: Role of task complexity, communication style, and brand personality](https://doi.org/10.1016/j.techfore.2024.123806). *Technological Forecasting and Social Change*, 209.
- Chaves, A. P., & Gerosa, M. A. (2021). [How should my chatbot interact? A survey on social characteristics in human–chatbot interaction design](https://arxiv.org/abs/1904.02743). *International Journal of Human–Computer Interaction*. [doi:10.1080/10447318.2020.1841438](https://doi.org/10.1080/10447318.2020.1841438) (preprint: arXiv:1904.02743).
- Salesforce. [10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/). Tip 10: "Map common off-topic or chit-chat queries to conversational, on-brand responses."
- Anthropic. [Claude Opus 4.7 system prompt](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (April 2026). On respecting a user's wish to end the conversation, and not becoming submissive with abusive users.
