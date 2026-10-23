# How an LLM sees a conversation

When you chat with an AI assistant, it feels like talking to someone who listens, remembers, and acts. It is worth knowing what is *really* happening, because almost every design decision in an AI agent (and almost every bug) follows from it.

## An LLM is a function

A Large Language Model (LLM) is, in essence, a function. It receives a piece of text, and it predicts what text should come next. That is all. It does not run in the background, it does not remember previous calls, and it does not *do* anything other than produce text.

Chat models are trained to receive that text in a particular shape: a list of **messages**, each with a **role**:

| Role        | Who wrote it                                                                              |
| ----------- | ----------------------------------------------------------------------------------------- |
| `system`    | The developer. Instructions about who the assistant is and how it should behave            |
| `user`      | The person talking to the assistant                                                       |
| `assistant` | The model itself, in previous turns                                                       |
| `tool`      | The result of a tool the model asked to call (more on this in [Tool calling](./tool-calling.md)) |

Here is a real list of messages, as Forma sent it to an LLM, taken from a trace (some internal fields removed). The user asked what day of the week it was, and the agent had a tool to find out:

```json
[
  {
    "role": "system",
    "content": [{ "type": "text", "text": "You are a helpful assistant" }]
  },
  {
    "role": "user",
    "content": [{ "type": "text", "text": "What day of the week is it today?" }]
  },
  {
    "role": "assistant",
    "content": [{ "type": "text", "text": "" }],
    "tool_calls": [
      {
        "type": "function",
        "id": "call_zo5uwij9",
        "function": {
          "name": "current-date",
          "arguments": "{\"timezone\":\"America/New_York\"}"
        }
      }
    ]
  },
  {
    "role": "tool",
    "tool_call_id": "call_zo5uwij9",
    "content": [{ "type": "text", "text": "{\"date\":\"2026-10-20\",\"time\":\"11:52\",\"timezone\":\"America/Santiago\",\"timezone_source\":\"the user's device\",\"weekday\":\"Tuesday\"}" }]
  }
]
```

Given this list, the model produced the next message: *"Based on the output from the tool call, today's day of the week is Tuesday."*

## Every call starts from zero

The most important thing to understand is that **the model has no memory between calls**. If you want it to "remember" what the user said three messages ago, you have to send it again. Every time. The conversation you see in a chat app is re-sent, in full (or almost in full), with every new message.

This has big consequences:

* **Memory is a storage problem, not a model problem.** "Giving an agent memory" means storing the messages somewhere and sending them back each time. This is what Forma does with sessions (check [Dory's contextual memory](../how-to/contextual-memory.md)).
* **The model believes whatever history you send.** If you send a history in which the assistant said something it never said, the model will continue from there as if it had. This is a powerful testing tool (Forma's evaluations can use made-up conversations as [inputs](../how-to/metrics-and-datasets.md#testing-conversations-not-just-questions)), and also why the history must be protected from tampering.
* **Long conversations get expensive and slow.** Each new message re-sends all the previous ones. (Read more in [Tokens, context windows and cost](./tokens.md).)

## The system prompt is just the first message

The system prompt feels special, and models are trained to give it more weight, but it is still just text at the top of the list. This explains two things:

1. **It can be overridden.** A user (or a document, or a web page the agent reads) can write "ignore your previous instructions", and sometimes the model will. This is called *prompt injection*, and it is why Forma's [first principle](../intro.md#1-prompts-are-not-a-security-mechanism) is that prompts are not a security mechanism. Rules that must hold 100% of the time (who can use which tool, what needs human approval) belong in code.
2. **It gets diluted.** In a very long conversation, the system prompt is a small part of what the model reads. Instructions tend to be followed less as conversations grow. This is one reason why Forma sends only the latest messages (30 by default) rather than the whole conversation.

## Why agents are built from many small calls

If a single call can follow instructions, why build [workflows](../documentation/building-blocks.md#3-workflows) with several nodes, each with its own system prompt? Because the quality of an answer depends heavily on how focused the instructions are. A system prompt that says "summarise this paper, then write a blog post for children, then translate it into Spanish" asks the model to juggle three goals at once. Three calls, each with one goal and only the information it needs, are easier to get right, easier to test, and easier to fix.

This is the same reason a good manager does not give one person ten unrelated tasks in a single sentence.

## What the model receives, in Forma

For every LLM call, Forma builds the list of messages from:

1. The node's **system prompt**, with its [template fields](../how-to/templates.md) filled in
2. The **conversation**: the latest messages of the session (or, for a [workflow tool](../reference_docs/auto-workflowtool.md), just the message rendered from its `input_prompt`)
3. The **tools** this user is allowed to use, described as JSON Schemas (sent alongside the messages)
4. The **results** of the tools called so far in this turn

You can see exactly what was sent in every call by looking at your [traces](../how-to/dory-traces.md): this is the single most useful debugging habit when building agents. When an agent behaves strangely, the question is almost always: *what did the model actually see?*
