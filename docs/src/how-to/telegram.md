# Your agent on Telegram

In this guide, you will turn Dory into a Telegram bot: you will find it in Telegram, write to it, and it will answer.

Telegram is the quickest channel to try. Creating a bot takes a minute, and needs no business account and no review. Like WhatsApp, Telegram does not run your agent: it forwards each message to a URL of your choice (a *webhook*), where Forma runs your agent and sends the answer back through the [Telegram Bot API](https://core.telegram.org/bots/api):

```text
You (Telegram) → Telegram → POST https://your-server/v1/chat → Forma runs your agent
You (Telegram) ← Telegram ← Bot API                          ← Forma sends the answer
```

Your agent behaves exactly as it does [on WhatsApp](./whatsapp.md): only the app changes.

You will need:

* Telegram, on your phone or computer
* The [Forma CLI](./getting-started.md#installing-the-forma-cli)
* [ngrok](https://ngrok.com/) (or a similar tool), so Telegram can reach the server running on your computer

## 1. Go to Telegram, and create a bot

Bots are created by talking to another bot, called [@BotFather](https://t.me/BotFather).

1. In Telegram, search for **@BotFather** (it has a blue check mark), open the chat, and press **Start**.
2. Send `/newbot`.
3. Choose a **name**. This is what people see in their chat list (e.g., `Dory`).
4. Choose a **username**. It must be unique, and end in `bot` (e.g., `dory_forma_bot`).

@BotFather answers with your bot's link and its **token**:

```text
Done! Congratulations on your new bot. You will find it at t.me/dory_forma_bot. [...]

Use this token to access the HTTP API:
7412345678:AAHf3x9Qz-Example-Token-Kp2Lm
Keep your token secure and store it safely, it can be used by anyone to control your bot.
```

> **✅ Check progress**: You have a token, and a `t.me/...` link to your bot.

> **❗Important**: Anyone with the token can control your bot. If it leaks, send `/revoke` to @BotFather to get a new one.

While you are there, you can also give your bot a description (`/setdescription`), shown to people before they start a chat, and a picture (`/setuserpic`).

## 2. Create the agent

```sh
forma init -p ./telegram-dory
cd telegram-dory
```

Tell Forma that its client is Telegram, by changing `runtime.yaml` to:

```yaml
client: telegram
storage: file
```

Then, add these two variables to the `.env` file of your project. They are named as Telegram names them:

```sh
TELEGRAM_BOT_TOKEN=7412345678:AAHf3x9Qz-Example-Token-Kp2Lm  # The token, from @BotFather
TELEGRAM_SECRET_TOKEN=Kq8mZ2vT9xLw4pRs                       # A password you make up
```

| Variable                | What it is                                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------------------------ |
| `TELEGRAM_BOT_TOKEN`    | Lets Forma send messages as your bot, and download the photos, voice notes and documents that people send   |
| `TELEGRAM_SECRET_TOKEN` | Any secret string of letters, numbers, `_` and `-` (e.g., the output of `openssl rand -hex 16`). You will give it to Telegram in step 4, and Telegram sends it back with every message, so nobody but Telegram can talk to your agent through your webhook |

> **📌 Note**: `.env` is git-ignored, so these secrets stay on your computer.

Dory uses [Ollama](https://ollama.com), so make sure it is running (check [Dory, your first Forma agent](./dory.md#3-chat-with-your-agent)).

> **📌 Note**: `forma dev` ignores the `client` in `runtime.yaml`, as it always serves its own chat UI. To talk to your agent through Telegram, you will use `forma serve`, which does use it. You can keep using `forma dev` to work on your prompts: the agent is the same in both.

## 3. Start the server, and make it public

```sh
forma serve
```

```text
Listening on 127.0.0.1:8080
```

Your server only accepts requests from your own computer, so Telegram cannot reach it yet. In a second terminal, open a tunnel with ngrok:

```sh
ngrok http 8080
```

ngrok prints a public address (yours will be different):

```text
Forwarding    https://1a2b3c4d.ngrok-free.app -> http://localhost:8080
```

Your **webhook URL** is that address followed by `/v1/chat`. For example, `https://1a2b3c4d.ngrok-free.app/v1/chat`.

## 4. Tell Telegram where your agent is

Register your webhook URL and your secret token with Telegram's `setWebhook` method. You only need to do this once (and again whenever the ngrok address changes):

```sh
export TELEGRAM_BOT_TOKEN=7412345678:AAHf3x9Qz-Example-Token-Kp2Lm
export TELEGRAM_SECRET_TOKEN=Kq8mZ2vT9xLw4pRs
curl "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/setWebhook?url=https://1a2b3c4d.ngrok-free.app/v1/chat&secret_token=$TELEGRAM_SECRET_TOKEN"
```

```json
{"ok":true,"result":true,"description":"Webhook was set"}
```

Telegram only accepts `https` URLs (on ports 443, 80, 88 or 8443), which is what ngrok gives you. A bot has a single webhook, so registering a new URL replaces the old one.

> **✅ Check progress**: Telegram answered `"Webhook was set"`.

## 5. Go to Telegram, and say hi

Open your bot's link (`t.me/dory_forma_bot`, in our example), press **Start**, and say hi. Dory will answer.

> **✅ Check progress**: Does Dory answer on Telegram?

Just like `forma dev`, `forma serve` reloads your agent every time you save a file. Change `src/prompts/agent_prompt.md`, and your next message will be answered by the new version. (Changes to `runtime.yaml` are the exception: they need a restart.)

> **💡 Did you notice?** Pressing **Start** sends `/start` as the first message of the chat. Your agent receives it like any other message, and usually answers it with a greeting. If you want a specific greeting, say so in your prompt (e.g., "If the user writes /start, introduce yourself in one sentence").

## What your agent receives

| When someone sends...                  | Your agent receives...                                |
| -------------------------------------- | ----------------------------------------------------- |
| A text                                 | The text                                              |
| A photo                                | The photo, followed by its caption (if any)           |
| A voice note, or an audio file         | The audio, as a file                                  |
| A document (e.g., a PDF)               | The document, followed by its caption (if any)        |
| A video, sticker, location or contact  | Nothing. These are ignored                            |

> **❗Important**: Receiving a photo, an audio or a document is not the same as understanding it: your agent's LLM must be able to read them. `llama3.1:8b`, the model `forma init` uses, only reads text. (Bots can download files of up to 20 MB.)

Your agent's answers are sent as text, in several messages when they are too long for one. They are written with Telegram's [Markdown formatting](https://core.telegram.org/bots/api#markdown-style), which supports bold (`*bold*`), italics (`_italics_`), code and links (`[text](https://...)`), but not tables or headings. (An answer Telegram cannot read as Markdown is sent as plain text.) Pictures produced by tools (e.g., [GoogleGenImage](../reference_docs/auto-googlegenimage.md)) are sent as photos and, when there is only one, the text is sent as its caption.

## Conversations and memory

Each Telegram chat is **one conversation** with your agent, and the chat's ID is used as the user's ID. As Telegram chats have no "new chat" button, the conversation never ends. Instead, the agent reads only the last 30 messages every time it answers. You can change this number in `runtime.yaml`:

```yaml
client: telegram
max_history_messages: 30
storage: file
```

With `storage: file`, conversations are kept in the `./forma-db` directory, as with `forma dev`.

People often split a thought into several messages. A message that arrives while the agent is still answering the previous one waits until it is done, and is answered then. And when Telegram is not sure a message arrived, it delivers it again: Forma recognises it, and answers it only once.

> **📌 Note**: If you add your bot to a group, the whole group is one conversation. Also, by default, Telegram only sends bots the group messages meant for them, like replies to their messages and commands (e.g., `/start@dory_forma_bot`). You can change this with `/setprivacy`, in @BotFather.

## Approvals (human in the loop)

When your agent wants to call a tool marked with `confirm_before_calling: true` (check [Human in the loop](./human-in-the-loop.md)), the person chatting receives the tool and its arguments, with two buttons:

```text
🔧 Issue refund

Amount: 12

Order id: C-9

Approve to continue
[ ✅ Approve ]   [ ✖️ Reject ]
```

Pressing a button answers the call, the buttons go away (so it cannot be answered twice), and the agent continues. If they write something instead, the action is rejected (it never runs), and the agent answers what they wrote. When the agent needs several approvals at once, each one gets its own message.

> **📌 Note**: On Telegram, the person who approves is the person chatting. Use `confirm_before_calling` for actions *they* should confirm (e.g., "book this appointment?"), not for those your staff should check.

Tools that run in your own client ([delegated tools](./human-in-the-loop.md#delegated-tools-when-the-client-knows-something-the-agent-doesnt)) cannot run on Telegram: the agent is told so, and carries on without them.

## Write for Telegram

Your agent can run on any channel unchanged, but a good answer on a website can be a bad one in a chat. Consider adding something like this to your prompt:

```text
You are chatting on Telegram. Keep your answers short, like a chat message.
Do not use tables or headings. For bold, use *single asterisks*.
```

Also, Telegram does not tell Forma the user's time zone. If your users are all in one place, add it to your `agent.yaml`, so the [`current-date`](../reference_docs/auto-currentdatetool.md) tool gets dates and times right:

```yaml
timezone: America/Santiago
```

## Going to production

In production, Forma agents are served by lean servers, published as container images, which read the same `runtime.yaml` as the CLI (check [Deployment](./deployment.md)). Your project's `Dockerfile` builds one with your agent. Keep `client: telegram`, and keep conversations in a database:

```yaml
client: telegram
storage: mongo
```

Then:

1. Set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_SECRET_TOKEN` (and `SESSIONS_DB_URL`) in your deployment. Production servers do not read `.env`.
2. Register your server's URL with `setWebhook`, as in [step 4](#4-tell-telegram-where-your-agent-is) (e.g., `https://your-server/v1/chat`), with the same secret token.

**Many agents in one server.** A server that runs one agent (like `forma serve`) needs nothing else. A [multi-tenant server](./deployment.md#multi-tenant-deployment), however, must know which agent each message is for, which it reads from the `X-Deployment-ID` header. Telegram cannot send this header, so a multi-tenant deployment needs a reverse proxy in front of Forma: give each bot its own webhook URL (e.g., `https://your-proxy/hook/telegram/{deployment_id}`), and have the proxy forward each request to Forma's `/v1/chat`, adding the header. The proxy must forward the `X-Telegram-Bot-Api-Secret-Token` header too.

In this setup, each deployment keeps its own credentials, instead of reading them from environment variables. Send them to the manager with the agent's definition, when you deploy it:

```json
{
  "definition": { ... },
  "credentials": {
    "telegram_api": {
      "bot_token": "7412345678:AAHf3x9Qz-Example-Token-Kp2Lm",
      "secret_token": "Kq8mZ2vT9xLw4pRs"
    }
  }
}
```

## If something does not work

Telegram tells you how your webhook is doing:

```sh
curl "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/getWebhookInfo"
```

```json
{"ok":true,"result":{"url":"https://1a2b3c4d.ngrok-free.app/v1/chat","has_custom_certificate":false,"pending_update_count":0,"max_connections":40,"ip_address":"203.0.113.7"}}
```

When deliveries fail, the result also includes a `last_error_message`, and `pending_update_count` grows: Telegram keeps the messages it could not deliver, and retries them.

| What you see                                                  | What to check                                                                                     |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `url` is empty                                                | The webhook was never set, or was removed. Run `setWebhook` again                                 |
| `last_error_message` mentions a connection or a host          | `forma serve` and ngrok are both running, and the `url` is ngrok's current address                |
| `last_error_message` is `Wrong response from the webhook: 401 Unauthorized` | The `secret_token` you gave `setWebhook` is not your `TELEGRAM_SECRET_TOKEN`            |
| `last_error_message` is `Wrong response from the webhook: 404 Not Found` | The URL ends in `/v1/chat`                                                              |
| `last_error_message` shows another status (e.g., `500`)       | The agent failed. Check the `forma serve` terminal                                                |
| The terminal shows your message, but no answer arrives        | Look for an error from the Bot API in the terminal (e.g., a revoked token)                        |
| The bot ignores your messages in a group                      | Reply to one of the bot's messages, or change its privacy with `/setprivacy` in @BotFather        |

To discard the messages Telegram is still retrying, register the webhook again, adding `&drop_pending_updates=true` to the URL. To stop receiving messages altogether, call `deleteWebhook`.
