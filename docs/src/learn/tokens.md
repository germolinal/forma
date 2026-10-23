# Tokens, context windows and cost

LLMs do not read characters or words. They read **tokens**: chunks of text, usually a few characters long. Everything about an LLM's limits and costs is measured in tokens, so it is worth understanding them.

## What is a token?

Before reaching the model, text is split into pieces by a *tokenizer*. Frequent words are often a single token (`the`, `bread`), while rare words, names, numbers and code are split into several (`sourdough` might be `s` + `our` + `dough`). As a rough rule, in English, **one token is about four characters, or three quarters of a word**. Other languages, especially those with different alphabets, often need more tokens for the same meaning (and therefore cost more).

Tokens explain some famous LLM weaknesses. A model that "sees" `strawberry` as two or three tokens does not naturally know how many `r`s it has, and long numbers split into arbitrary chunks are one reason why models make arithmetic mistakes. This is why agents should call tools for things like counting, maths, or dates, rather than "thinking" them.

## Input and output tokens

Every LLM call has:

* **Input tokens**: everything the model reads. The system prompt, the conversation, the tool descriptions, and the tool results.
* **Output tokens**: everything the model writes. Usually much fewer, but often several times more expensive per token.

Forma reports both at the end of every request (the last line of the [`sse` stream](../how-to/http-api.md#responses-sse)):

```json
{"tokens":[{"provider":"ollama","model":"llama3.1:8b","calls":2,"input_tokens":336,"cached_input_tokens":65,"output_tokens":39, ...}],"tools":["current-date"]}
```

## Conversations get more expensive with every message

Remember that the model [has no memory](./conversations.md#every-call-starts-from-zero): every new message re-sends the conversation so far. Here is a real five-message conversation with Dory, and the input tokens of each request:

| Message                                   | Input tokens | Of which, cached | Output tokens |
| ----------------------------------------- | -----------: | ---------------: | ------------: |
| Hi! I am planning a picnic.               |           28 |                7 |            49 |
| What food should I bring for 4 people?    |           96 |               76 |           283 |
| And drinks?                               |          391 |              378 |           291 |
| What if it rains?                         |          696 |              681 |           349 |
| Summarise the plan in one line.           |        1,063 |            1,044 |            43 |

The last question is only six words long, but it cost 1,063 input tokens, because the whole picnic plan was sent along with it. Over the conversation, the model read 2,274 input tokens to answer five short questions. The longer the answers, the faster this grows.

This is why Forma sends only the latest messages to the model (30 by default, set with `max_history_messages` in the [runtime settings](../reference_docs/auto-formaruntime.md)), and why concise answers are not only nicer to read, but also cheaper.

## Cached tokens

Notice the *cached* column above. Because each request starts with exactly the same text as the previous one (the same system prompt, and the same conversation so far), the provider can reuse the work it already did for that beginning. These **cached input tokens** are processed faster and, with most cloud providers, billed at a large discount.

You get the most out of caching by keeping the beginning of your requests stable:

* Put what never changes (instructions, policies, tool descriptions) first, in the system prompt, and avoid putting things that change on every request (like the current time) there. This is why Forma gives agents the date through the [`current-date`](../reference_docs/auto-currentdatetool.md) tool, instead of writing it into the prompt.
* Avoid editing a conversation's history: changing an early message invalidates the cache for everything after it.

## The context window

Every model has a maximum number of tokens it can read in a single call: its **context window**. Modern models accept from a few thousand to over a million tokens. Exceeding it makes the request fail (or makes the provider silently cut the oldest content).

A large context window does not mean you should fill it, though:

* **Cost and latency grow with input.** You pay for every token, on every call.
* **Attention gets diluted.** Models are better at using information at the beginning and end of their input than in the middle, and instructions are followed less reliably as the input grows. A focused prompt with only the relevant information usually beats a huge one with everything.

This is one of the reasons behind Forma's design: [workflows](../documentation/building-blocks.md#3-workflows) give each node only what it needs, [workflow tools](../reference_docs/auto-workflowtool.md) run in a fresh session without the conversation, and tools like workspaces let the agent *read* documents when needed, instead of pasting them all into the prompt.

## Estimating what an agent costs

A rough estimate of the cost of one request is:

```text
cost ≈ Σ over every LLM call ( input tokens × input price + output tokens × output price )
```

And the important part is *every LLM call*. One user message may involve many:

* One triage call per node, plus one more after each round of tools
* The calls of every node in a [workflow tool](../how-to/tools-introduction.md)
* A summarisation call, if a node reaches its `max_iterations`

The `calls` field in Forma's usage report tells you how many there were (in the example above, `2`: one to decide to call the `current-date` tool, and another to answer with its result). Watch it as your agent grows: a design change that adds a node adds at least one call to *every* request.
