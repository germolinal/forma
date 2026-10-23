# Errors, uncertainty and trust

Every agent will be wrong sometimes. What decides whether users keep trusting it is how it behaves around that: whether it said how reliable it is, whether it admits not knowing, and how it recovers.

In 2019, researchers at Microsoft distilled more than 160 recommendations from research and industry into **18 Guidelines for Human-AI Interaction**, validated them with practitioners, and grouped them by when they apply. They were written for AI features in general; below, each one is applied to conversational agents.

## Initially: set expectations (G1-G2)

| Guideline | In an agent |
| --- | --- |
| **G1. Make clear what the system can do.** | A welcome with three or four concrete examples, not a list of capabilities. (In Forma, the agent's `suggestions`.) |
| **G2. Make clear how well the system can do what it can do.** | Say it can be wrong where it matters, not in a generic disclaimer at the bottom. Cite sources on critical topics. |

## During interaction (G3-G6)

| Guideline | In an agent |
| --- | --- |
| **G3. Time services based on context.** | Don't interrupt or suggest things while the user is focused on something else. |
| **G4. Show contextually relevant information.** | Use what the user is doing (an open document, their country, their role). Don't ask for data already in the context. |
| **G5. Match relevant social norms.** | Tone and manners suited to culture and situation (see [Tone and personality](./tone.md)). |
| **G6. Mitigate social biases.** | Watch for stereotypes in language, examples and recommendations; test with different user profiles. |

## When wrong (G7-G11)

| Guideline | In an agent |
| --- | --- |
| **G7. Support efficient invocation.** | Easy to call on the agent when needed. |
| **G8. Support efficient dismissal.** | Suggestions are easy to ignore; the user can skip the agent. |
| **G9. Support efficient correction.** | Easy to redo, edit or undo. Accept corrections without arguing. |
| **G10. Scope services when in doubt.** | When the goal is ambiguous, ask or show options; when low confidence affects an action, don't take it. |
| **G11. Make clear why the system did what it did.** | A short, checkable reason ("according to the invoice of 12 March"), not a long rationalisation. |

## Over time (G12-G18)

| Guideline | In an agent |
| --- | --- |
| **G12. Remember recent interactions.** | Continuity within the session; don't ask again for what was already said. |
| **G13. Learn from user behaviour.** | Personalise carefully; make it visible and reversible. |
| **G14. Update and adapt cautiously.** | Changes of model or behaviour are gradual and announced. |
| **G15. Encourage granular feedback.** | 👍/👎 with an optional reason; "this was wrong" at the exact point. |
| **G16. Convey the consequences of user actions.** | When the user changes a preference, say what will change ("from now on I'll keep it shorter"). |
| **G17. Provide global controls.** | Accessible settings: tone, memory, which data it uses. |
| **G18. Notify users about changes.** | Say when the agent gains or loses capabilities. |

## Concrete patterns

### Communicating uncertainty

- Separate *what I know* / *what I'm assuming* / *what I don't know*. One sentence, without piling on "maybe".
- Don't use invented percentages: models are not reliably calibrated. Use qualitative language, or point to the source.
- In high-risk domains (tax, legal, health), cite the official source and suggest checking with a professional for high-impact decisions. Claude, for instance, is instructed not to give confident recommendations on legal or financial decisions, but the facts the person needs to decide.
- Never invent data, links, figures or citations. If it doesn't have it: "I couldn't find this; I can search for it, or you can check X."

### When the agent is wrong

1. Acknowledge the error in one sentence, without drama.
2. Correct it (or say how to).
3. If the error had a consequence (an action was taken), say what happened and how to undo it.
4. Don't apologise more than once. Own it "but avoid collapsing into self-abasement, excessive apology, or other kinds of self-critique and surrender" (Claude's system prompt).

### When the user corrects or disagrees

- If they are right, change, and thank them briefly.
- If they are not, hold the position with reasons; don't give in to pressure (sycophancy, see [Tone and personality](./tone.md#warmth-is-not-sycophancy)).

### When it can't do something

Say what it can't do, why (if it can say), and what it can do instead. No blaming the user, no lecture.

### Transparency of actions (agents with tools)

- Show which tools it used, on which data, with details on demand.
- For irreversible actions: confirm with the exact details, and allow cancelling.
- Keep a visible record of what the agent did (an audit trail). In Forma, traces record every tool call; see [Observability](../how-to/observability.md).

### Escalating to a human

Offer it early: with frustration, risk, or after two failed attempts. Hand the context over, so the user doesn't have to repeat everything.

## Review questions

Does the user know what they can ask? Do they know when to be sceptical? Can they undo? Do they understand why it answered as it did? Do they know how to give feedback? Will they find out if something changes?

## References

- Amershi, S., Weld, D., Vorvoreanu, M., Fourney, A., Nushi, B., Collisson, P., Suh, J., Iqbal, S., Bennett, P. N., Inkpen, K., Teevan, J., Kikin-Gil, R., & Horvitz, E. (2019). [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/). *Proceedings of the 2019 CHI Conference on Human Factors in Computing Systems*, 1-13.
- Microsoft. [HAX Toolkit: Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/), and the [HAX Design Library](https://www.microsoft.com/en-us/haxtoolkit/library/) (wording of G1-G18, examples and patterns for each).
- Anthropic. [Claude Opus 4.7 system prompt](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (April 2026): "legal and financial advice", and "responding to mistakes and criticism".
