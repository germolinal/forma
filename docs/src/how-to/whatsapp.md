# Your agent on WhatsApp

In this guide, you will put Dory on WhatsApp: you will write to it from your own phone, and it will answer.

WhatsApp does not run your agent. It forwards each message to a URL of your choice (a *webhook*), where Forma runs your agent and sends the answer back through the [WhatsApp Cloud API](https://developers.facebook.com/documentation/business-messaging/whatsapp/about-the-platform):

```text
You (WhatsApp) → Meta → POST https://your-server/v1/chat → Forma runs your agent
You (WhatsApp) ← Meta ← WhatsApp Cloud API             ← Forma sends the answer
```

You will need:

* A phone with WhatsApp
* A Facebook account, to sign in to [Meta for Developers](https://developers.facebook.com)
* The [Forma CLI](./getting-started.md#installing-the-forma-cli)
* [ngrok](https://ngrok.com/) (or a similar tool), so Meta can reach the server running on your computer

> **📌 Note**: You do not need a new phone number. Meta gives you a free test number to develop with.

## 1. Create the agent

```sh
forma init -p ./whatsapp-dory
cd whatsapp-dory
```

Then, tell Forma that its client is WhatsApp, by changing `runtime.yaml` to:

```yaml
client: whatsapp
storage: file
```

Dory uses [Ollama](https://ollama.com), so make sure it is running (check [Dory, your first Forma agent](./dory.md#3-chat-with-your-agent)).

> **📌 Note**: `forma dev` ignores the `client` in `runtime.yaml`, as it always serves its own chat UI. To talk to your agent through WhatsApp, you will use `forma serve`, which does use it. You can keep using `forma dev` to work on your prompts: the agent is the same in both.

## 2. Go to Meta, and create a WhatsApp app

1. Go to [developers.facebook.com/apps](https://developers.facebook.com/apps) and click **Create app**. (The first time, Meta will ask you to register as a developer.)
2. Give your app a name, and choose the **Connect with customers through WhatsApp** use case.
3. Choose a business portfolio, or create one (it can be just you), and finish creating the app.
4. In the **Quickstart** page, click **Start using the API**. This opens **API Setup**, where you will find everything you need for now.

In **API Setup**:

1. Select a WhatsApp Business account, or create one.
2. In **From**, select the test number, and copy its **Phone number ID**. (This is an ID, not the phone number itself.)
3. Click **Generate access token**, and copy the token.
4. In **To**, add your own WhatsApp number. Meta will send a code to it, to check that the number is yours. The test number can only send messages (and, therefore, answers) to the numbers in this list.
5. Click **Send message**. A "Hello World" message should arrive on your phone, from the test number.

Finally, go to **App settings > Basic**, click **Show** next to **App secret**, and copy it. Meta signs every message it sends you with it, which is how Forma knows that a message really comes from Meta.

> **✅ Check progress**: You received a message on WhatsApp, and you have a Phone number ID, an access token and the App secret.

> **❗Important**: This access token is temporary, and expires within a day. That is fine for now. Check [Going to production](#going-to-production) for one that does not expire.

## 3. Give Forma the credentials

Add these four variables to the `.env` file of your project. They are named as Meta's dashboard names them:

```sh
WHATSAPP_PHONE_NUMBER_ID=106540352242922  # The Phone number ID, from API Setup
WHATSAPP_ACCESS_TOKEN=EAAJb...            # The access token, from API Setup
WHATSAPP_APP_SECRET=4f2b9c...             # The App secret, from App settings > Basic
WHATSAPP_VERIFY_TOKEN=Kq8mZ2vT9xLw4pRs    # A password you make up
```

| Variable                   | What it is                                                                                                   |
| -------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `WHATSAPP_PHONE_NUMBER_ID` | The number your agent speaks as. Forma sends its answers from it, and ignores messages sent to other numbers |
| `WHATSAPP_ACCESS_TOKEN`    | Lets Forma send messages, and download the photos, voice notes and documents that people send               |
| `WHATSAPP_APP_SECRET`      | Forma refuses every message whose signature does not match it, so nobody but Meta can talk to your agent through your webhook |
| `WHATSAPP_VERIFY_TOKEN`    | Any secret string (e.g., the output of `openssl rand -hex 16`). You will type it again in Meta, in step 5    |

> **📌 Note**: `.env` is git-ignored, so these secrets stay on your computer.

## 4. Start the server, and make it public

```sh
forma serve
```

```text
Listening on 127.0.0.1:8080
```

Your server only accepts requests from your own computer, so Meta cannot reach it yet. In a second terminal, open a tunnel with ngrok:

```sh
ngrok http 8080
```

ngrok prints a public address (yours will be different):

```text
Forwarding    https://1a2b3c4d.ngrok-free.app -> http://localhost:8080
```

Your **webhook URL** is that address followed by `/v1/chat`. For example, `https://1a2b3c4d.ngrok-free.app/v1/chat`.

> **📌 Note**: Leave both terminals open. If the ngrok address changes (e.g., because you restarted it), update it in Meta, as in the next step.

## 5. Go to Meta, and connect the webhook

1. In your app's dashboard, go to **Use cases**, click **Customize** on the WhatsApp use case, and open **Configuration**. (In older apps, it is **WhatsApp > Configuration**.)
2. In **Callback URL**, paste your webhook URL (the one ending in `/v1/chat`).
3. In **Verify token**, paste the value of your `WHATSAPP_VERIFY_TOKEN`.
4. Click **Verify and save**. Meta will call your server, which only answers if the verify token matches. You will see this request in the `forma serve` terminal.
5. Under **Webhook fields**, subscribe to **messages**. Until you do, Meta will not send you any messages.

> **✅ Check progress**: Meta saved your Callback URL, and you are subscribed to `messages`.

## 6. Go to WhatsApp, and say hi

Open WhatsApp on your phone, and write to the test number (the chat where "Hello World" arrived). Dory will answer.

> **✅ Check progress**: Does Dory answer on WhatsApp?

Just like `forma dev`, `forma serve` reloads your agent every time you save a file. Change `src/prompts/agent_prompt.md`, and your next WhatsApp message will be answered by the new version. (Changes to `runtime.yaml` are the exception: they need a restart.)

## What your agent receives

| When someone sends...                 | Your agent receives...                                            |
| ------------------------------------- | ----------------------------------------------------------------- |
| A text                                | The text                                                          |
| A photo                               | The photo, followed by its caption (if any)                       |
| A voice note, or an audio file        | The audio, as a file                                              |
| A document (e.g., a PDF)              | The document, followed by its caption (if any)                    |
| A reaction (e.g., 👍)                 | Nothing. The agent does not answer reactions                      |
| A video, sticker, location or contact | Nothing. These are ignored                                        |

> **❗Important**: Receiving a photo, an audio or a document is not the same as understanding it: your agent's LLM must be able to read them. `llama3.1:8b`, the model `forma init` uses, only reads text.

Your agent's answers are sent as text, in several messages when they are too long for one. Pictures produced by tools (e.g., [GoogleGenImage](../reference_docs/auto-googlegenimage.md)) are sent as WhatsApp images and, when there is only one, the text is sent as its caption.

## Conversations and memory

Each phone number has **one conversation** with your agent. The phone number is the user's ID and, as WhatsApp has no "new chat" button, the conversation never ends. Instead, the agent reads only the last 30 messages every time it answers. You can change this number in `runtime.yaml`:

```yaml
client: whatsapp
max_history_messages: 30
storage: file
```

With `storage: file`, conversations are kept in the `./forma-db` directory, as with `forma dev`.

People often split a thought into several messages. A message that arrives while the agent is still answering the previous one waits until it is done, and is answered then. And when Meta is not sure a message arrived, it delivers it again: Forma recognises it, and answers it only once.

## Approvals (human in the loop)

When your agent wants to call a tool marked with `confirm_before_calling: true` (check [Human in the loop](./human-in-the-loop.md)), the person chatting receives the tool and its arguments, with two buttons:

```text
🔧 Issue refund

Amount: 12

Order id: C-9

Approve to continue
[ ✅ Approve ]   [ ✖️ Reject ]
```

Pressing a button answers the call, and the agent continues. If they write something instead, the action is rejected (it never runs), and the agent answers what they wrote. When the agent needs several approvals at once, each one gets its own message.

> **📌 Note**: On WhatsApp, the person who approves is the person chatting. Use `confirm_before_calling` for actions *they* should confirm (e.g., "book this appointment?"), not for those your staff should check.

Tools that run in your own client ([delegated tools](./human-in-the-loop.md#delegated-tools-when-the-client-knows-something-the-agent-doesnt)) cannot run on WhatsApp: the agent is told so, and carries on without them.

## Write for WhatsApp

Your agent can run on any channel unchanged, but a good answer on a website can be a bad one on a phone. Consider adding something like this to your prompt:

```text
You are chatting on WhatsApp. Keep your answers short, like a chat message.
Do not use tables or headings. For bold, use *single asterisks*.
```

Also, WhatsApp does not tell Forma the user's time zone. If your users are all in one place, add it to your `agent.yaml`, so the [`current-date`](../reference_docs/auto-currentdatetool.md) tool gets dates and times right:

```yaml
timezone: America/Santiago
```

## Going to production

**Your server.** In production, Forma agents are served by lean binaries, which read the same `runtime.yaml` as the CLI (check [Deployment](./deployment.md)). Keep `client: whatsapp`, and keep conversations in a database:

```yaml
client: whatsapp
storage: mongo
```

Production servers do not read `.env`: set the four `WHATSAPP_*` variables (and `SESSIONS_DB_URL`) in your deployment.

**Meta.** The test number and the temporary token are meant for development. Before real people can talk to your agent:

* **Get a permanent token.** In your business portfolio's settings, create a *system user*, give it access to your app and to your WhatsApp Business account, and generate a token for it with the `whatsapp_business_messaging` permission.
* **Add your own phone number** in **API Setup**, and follow Meta's steps to register it.
* **Publish your app**, and change its **Callback URL** to your server's.

These steps change from time to time, so check Meta's [Get started guide](https://developers.facebook.com/documentation/business-messaging/whatsapp/get-started) for the details.

**Many agents in one server.** A server that runs one agent (like `forma serve`) needs nothing else. A [multi-tenant server](./deployment.md#multi-tenant-deployment), however, must know which agent each message is for, which it reads from the `X-Deployment-ID` header. Meta cannot send this header, so a multi-tenant deployment needs a reverse proxy in front of Forma: give each agent its own webhook URL (e.g., `https://your-proxy/hook/whatsapp/{deployment_id}`), and have the proxy forward each request to Forma's `/v1/chat`, adding the header. As Forma checks Meta's signature, the proxy must forward the `X-Hub-Signature-256` header, and the body exactly as it received it.

In this setup, each deployment keeps its own WhatsApp credentials, instead of reading them from environment variables. Send them to the manager with the agent's definition, when you deploy it:

```json
{
  "definition": { ... },
  "credentials": {
    "whatsapp_api": {
      "phone_number_id": "106540352242922",
      "access_token": "EAAJb...",
      "app_secret": "4f2b9c...",
      "verify_token": "Kq8mZ2vT9xLw4pRs"
    }
  }
}
```

## If something does not work

| What you see                                                   | What to check                                                                                                         |
| -------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Meta cannot validate your Callback URL                         | `forma serve` and ngrok are both running; the URL ends in `/v1/chat`; the verify token is exactly your `WHATSAPP_VERIFY_TOKEN` |
| You write to the test number, but nothing shows in the terminal | You are subscribed to `messages`, and the ngrok address in Meta is the current one                                   |
| Every message is refused with a `401` (`X-Hub-Signature-256 does not match`) | `WHATSAPP_APP_SECRET` is not the App secret of the app the webhook belongs to                              |
| The terminal shows your message, but no answer arrives         | Look for an error from `graph.facebook.com` in the terminal. Most often, the temporary access token expired (generate a new one), or your number is not in the **To** list |
| The agent says it cannot see your photo                        | Your agent's LLM only reads text                                                                                      |
