# Dory's contextual memory

[Short-term or contextual memory](../documentation/memory.md) is essential for multi-turn chatbots. Without it, conversations are terribly frustrating and the user experience is plain bad. (This does not apply for single-turn AI Agents; for example, those that just summarise text.)

In the previous tutorial you might have noticed that Dory already remembers what you said earlier in the conversation. So, in this section we will see *where* those memories are kept, and what changes when you deploy Dory.

As outlined in the [Architecture](../documentation/architecture.md) section, Forma agents are stateless. This means that they do not have any memory themselves, and thus the conversation needs to be stored somewhere else: a file, or a database.

> **Note**: If you are interested in keeping the messages on the client, you might be intereseted in [this example](../examples/clients/vercel-aisdk-5-setup.md).

## 1. During development, conversations are kept in files

When using the `forma` CLI (i.e., `forma dev` or `forma serve`), conversations are stored in a directory called `forma-db`, in the root of your project. It was created the first time you ran `forma dev`:

```text
forma-db/
  index.json
  msg_index.json
  sessions/
    7c07c166-ee31-4339-83a1-96885df45dfd.json  <- one file per conversation
```

There is nothing to set up, and nothing to run. Every message you send, as well as Dory's responses, will be stored in these files.

> **📌 Note**: `forma-db` is listed in your `.gitignore` file, as conversations should not be shared with your team through `git`.

## 2. Chat and see your conversation

Run `forma dev`, open [http://127.0.0.1:8080](http://127.0.0.1:8080) and tell Dory your name. Then, ask it what your name is.

```sh
forma dev
```

Now open the newest file in `forma-db/sessions`. You should find something like this (some fields removed for brevity):

```json
{
  "role": "user",
  "content": [{ "type": "text", "text": "hey, my name is Peter" }],
  "user_id": "forma-user",
  "status": "sent"
},
{
  "role": "assistant",
  "content": [{ "type": "text", "text": "Nice to meet you, Peter! How are you doing today?" }],
  "user_id": "forma-user",
  "status": "sent"
}
```

Every time you send a message, Forma reads the latest messages of that conversation from this file, and sends them to the LLM along with your new message. This is how Dory "remembers".

> **📌 Note**: Click **New chat** in the chat UI to start a new conversation (i.e., a new session) from scratch.

## 3. How much does Dory remember?

Forma does not send the *whole* conversation to the LLM every time. By default, it retrieves the last 30 messages. This prevents the system prompt from being diluted by a large number of messages, and it reduces the token usage.

You can change this in the `runtime.yaml` file, using the `max_history_messages` field:

```yaml
client: sse
max_history_messages: 10 # <-- HERE!
storage: file
```

> **❗Important**: Changes to `runtime.yaml` are not hot-reloaded. You need to stop and restart `forma serve` for them to take effect. (`forma dev` ignores this file, as explained in the [Runtime reference](../reference_docs/auto-formaruntime.md).)

## 4. What about production?

Files are great for development, but not for production, where you might run several copies of your agent at once. In production, conversations are normally stored in a MongoDB-compatible database, whose connection string is given through the `SESSIONS_DB_URL` environment variable. This is explained in the [Deployment](./deployment.md) section.

## What is next? We scale! 🚀

Dory is a very simple AI agent. In fact, it can be considered an LLM Wrapper: it sends a message to an LLM, and it responds it. But from here, things will become more complex. We will add tools and workflows, allowing for a complex processing logic for each request.

The next section is about [Observability](./dory-traces.md), which will let us understand what happens on every request. Getting into the habit of reading observability reports is very important for two reasons:

1. Because, **once you deploy any AI agent, you do not get access to the code** any more (this is true for nearly all frameworks, not just Forma). All you can do to monitor what is happening when your real users/clients interact with your agent is through observability.
2. Because with Forma—contrary to other frameworks—**your experience during development is nearly identical to your experience after deployment**. That is to say, you do not get to see what the code is doing exactly, but you do get to see what happens within the agent thanks to observability.

See you in [the next section](./dory-traces.md)!
