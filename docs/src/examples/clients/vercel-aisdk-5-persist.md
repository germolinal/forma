# Vercel AI-SDK v5 - With session persistence

> **Note**: This guide is the continuation of the [other Vercel AI-SDK v5 tutorial](./vercel-aisdk-5-setup.md). 

We already mentioned that the other [other Vercel AI-SDK v5 tutorial](./vercel-aisdk-5-setup.md):
1. Keeps the history of messages on the browser, not in the backend
2. Sends the whole chat history to the server on every interaction

While those to elements can make an AI agent cheaper (because you do not need to manage an Sessions database), they also imply that (respectively):

1. **Sessions are relatively ephemeral**, and will disappear when the user refreshes the page or deletes their cookies, depending on how memory is implemented.
2. **Every new message sends much more information**, meaning that you are consuming more data; but also, that if someone intercepts this communication, they not only get a loose message (e.g., "Sure, that sounds good!") but the whole conversation surrounding that specific message (e.g., they would know "what" sounds good).

The solution is to keep the memory on the back-end. That way, when you send a message, the Forma Agent will first retrieve the context of the conversation (i.e., the previous messages) and then send them to the LLM. Then, before returning the answer to you, it will update the sessions database by adding the response. 

Let's do this.

## Setting up your Forma AI Agent so respect this communication protocol

Forma always stores sessions, so the runtime does not change:

```yaml
# runtime.yaml
client: ai-sdk-v5 # <-- NOT CHANGED
storage: file
```

Also, remember to setup an API Key. Add this to the `.env` file in your Forma directory:

```sh
# .env, within the Forma agent directory
FORMA_AGENT_KEY=fake-key-which-should-be-longer-in-production

# In production, you'd need to set the SESSIONS_DB_URL=mongodb://root:example@localhost:27017/?directConnection=true

```

There are two main things we need to do now:

1. Making sure we let the Forma agent know that a new session will start
2. Make sure that new chat messages contain information about who the user is, and which session this message belongs to.

## Initializing sessions

### Authenticating the user

When a website opens, you need to authenticate the user. We will not do this here, but we can emulate something like that.

The first thing is to give the Chat some place to store the user information; in other words, to keep the user ID in the state.

```ts
// Add this within the Home() component in app/page.tsx.

// You will need to import `useState`
const [userId, setUserId] = useState<string | undefined>(undefined)
```

Then, make sure the user is initialized properly when the page loads.

```ts
// Add this within the Home() component in app/page.tsx.

// import useEffect
useEffect(() => {
  // You need to handle this with 
  // your own authentication service
  setUserId('my-user-which-i-will-authenticate')
}, [])
```

### Create a new session when the user logs in

Once a user logs in, we need to create a new session for them. We do this by reaching the `v1/init` endpoint. The way to do this on the front-end is the following.

First, give the Chatbot some place to put the session id (i.e., state):

```ts
const [sessionId, setSessionId] = useState<string | undefined>(undefined)
```

Then, ask the Forma Agent to initialize and give you a new session-id when the user logs in:


```ts
// This function creates a new session, and gets the session/
const getSessionId = useCallback(async () => {
    // Let's not create a session if we have no user or session IDs
    if (!userId) {
      return
    }

    // Send this information to the back-end to create the session
    let r = await fetch(`/api/init`, {
      headers: {
        // Give this information to the back end
        "x-user-id": userId
      }
    })
    let res = await r.json()
    if (!r.ok) {
      console.warn(res)
    } else {
      setSessionId(res.session_id)
    }
}, [userId, setSessionId])

// This will runn every time the userId changes
useEffect(() => {
    getSessionId()
}, [userId])
```

And then we need to implement the back-end of our own client, where we will also pass the authentication information:

```ts
// This file is app/api/init/route.ts

import { NextRequest, NextResponse } from "next/server";

export async function GET(request: NextRequest) {
    let userId = request.headers.get("x-user-id")
    if (!userId) {
        return new NextResponse(JSON.stringify({
            error: "we cannot create a session for an unknown user",
            status: 400
        }), { status: 400 });
    }

    const headers = new Headers();
    // add authentication
    
    headers.set("Authorization", `Bearer ${process.env.FORMA_AGENT_KEY!}`);    
    // Add the user id, which will own the newly created session
    headers.set("X-User-Id", userId);

    try {
        let r = await fetch(`${process.env.FORMA_AGENT_URL!}/v1/init`,
            { headers },
        )
        if (!r.ok) {
            let error = await r.text()
            console.error(error)
            return new NextResponse(JSON.stringify({
                status: r.status,
                error: error
            }), { status: r.status });
        }
        return new NextResponse(r.body);
    } catch (error) {
        console.error(error)
        return new NextResponse(JSON.stringify({
            error: "Failed to get sessionId from Forma agent",
            status: 500
        }), { status: 500 });
    }
}
```

## Add `userId` and `sessionId` to the message sent to Forma

First, we should add this information on the front-end (i.e., Browser). We do this by updating the `SubmitForm` component, which is the one in charge of that. Turn it into this:

```ts
// Update this component, within app/page.tsx
function SubmitForm({ sendMessage, userId, sessionId }: {
  sendMessage: (message: { text: string }, options?: ChatRequestOptions) => Promise<void>,
  userId?: string, // <- Add this as an argument
  sessionId?: string // <- Add this as an argument
}) {
  const [input, setInput] = useState<string>("")
  const textAreaRef = useRef<HTMLTextAreaElement>(null)
  const submit = useCallback(async (text: string) => {
    let area = textAreaRef.current
    if (area) {
      area.value = ""
    }
    sendMessage({ text }, {      
      headers: {
        "x-user-id": userId || "", // <- We pass userId in the header
        "x-session-id": sessionId || "" // <- Also, sessionId in the header
      }
    })
  }, [sendMessage, userId, sessionId])

  const onKey = useCallback((e: any) => {
    let newv = e.target.value.trim()
    setInput(newv)
    if (e.key === 'Enter') {
      submit(newv)
    }
  }, [setInput, submit]) // <- We also updated this. Search for docs on React's useCallback

  return <div className='h-fit'>
    <div className='flex items-center p-2 rounded-2xl border m-1'>
      <textarea ref={textAreaRef} onKeyUp={onKey} className='flex-grow outline-none focus:outline-none resize-none' placeholder="Ask me anything!" />
      <Button size="icon" aria-label="Submit" onClick={() => {
        submit(input)
      }} >
        <ArrowUpIcon />
      </Button>
    </div>
  </div>
}
```

We added new arguments to `SubmitForm`, so we need to adjust this section:

```ts
return (
  <main className='flex flex-col h-screen w-full max-w-3xl mx-auto overflow-hidden'>
    <Chatlog messages={messages} />
    <SubmitForm 
      sendMessage={sendMessage}  
      userId={userId} // <- pass user id
      sessionId={sessionId} // <- pass session id
    />
  </main>
);
```

Then you need to read this information on the back-end. You can do this by adding the following at the top of the `POST` function:

```ts
// Add this within `app/api/chat/route.ts, at the top of the `POST` function
let userId = request.headers.get("x-user-id")
let sessionId = request.headers.get("x-session-id")
if (!userId || !sessionId) {
    // Make sure headers are passed.
    return new NextResponse(JSON.stringify({
        error: "sending message without user or session id is not allowed",
        status: 400
    }), { status: 400 });
}
```

And then forwarding this information to the Forma agent

```ts
// Update this part of `app/api/chat/route.ts, within the `POST` function
const r = await fetch(`${process.env.FORMA_AGENT_URL!}/v1/chat`, {
    method: 'POST',
    headers: {
        "Content-Type": "application/json",
        "Authorization": `Bearer ${process.env.FORMA_AGENT_KEY!}`
        "X-User-Id": userId, // <-- Add this
        "X-Session-Id": sessionId // <-- And this
    },
    body: JSON.stringify(body),
    cache: 'no-store',
});
```


## Send just the last message.

In order to only send the last message, we need to configure the `useChat` hook we used previously. It should now look like this:

```ts
// file: `app/page.tsx`
// Update the call to `useChat` to this:
const {
  messages,
  sendMessage,
} = useChat<UIMessage>({
  // Import DefaultChatTransport from 'ai'
  transport: new DefaultChatTransport({
    prepareSendMessagesRequest: ({ messages }) => {
      return {
        body: messages.at(-1)!, // <-- Send only the last message
      }
    }
  }),
  onError: (e) => {
    console.warn(e)
  }
});

```

## Run it all together

You should be able to run all of it by running these three elements in different terminal tabs:


```sh
# Go to wherever your forma agent is
cd <forma/path>
# Run the 
docker-compose -f dev/sessions-db.yaml up
```

```sh
# Go to wherever your forma agent is
cd <forma/path>
# The Forma CLI
forma serve
```

```sh
# Run the Web App you just made
npm run dev
```
