# Role Based Access

Role-based access is a feature that allows you to let some users do some things that other users cannot do. This is already very common on web applications, where developers assign *Roles* to users for cost, security or other concerns. For instance, you can have a role called `free-tier-user` which grants permissions to a subset of features. Similarly, an `admin` role might let users read/write data from a datbase that is inaccessible by other users. **Forma allows you to implement Role-based-access for the tools in your agent**.

> **Note:** as mentioned in the [architecture](../documentation/architecture.md) section, Forma will not authenticate your users. A different component within your application should make sure the permissions and user ID given to forma are real.

## How it is implemented

Forma will handle the Role-based acess in the following way:

1. You provide the roles of the user in the header of the request, follows: `curl -H 'X-User-Roles: paid-user,admin' ...`. (If no roles are provided, the user is assumed to have NO roles and thus they won't have access to any tool that is restricted.)
2. Forma parses those, and—at every triage-stage of each node, at run time—will select the tools available for users with the specified roles.
3. A request is made to the LLM including only this subset of tools.
4. The path follows as usual

This approach intends to eliminate the chances of an LLM calling a tool that should not be allowed to the user. An alternative could have been, for example, to add an instruction saying something like *"this user is in the Free tier, they do not have write access"*, but this is not reliable enough for a security-sensitive application.

Another consequence of this is that, **if a user intents to call a function they have no access** to (e.g., a free-user asking for a paid feature), **there will be no error**. Forma will simply eliminate the restricted tools when asking the LLM to choose an action, and the LLM will not even be aware that such a tool exists.

## Limiting access to tools

Restricting the use of a tool to specific roles is as simple as passing the `just_for` field to it. Let's remember the blog-writer agent we [developed earlier](./tools-introduction.md).

```yaml 

name: write-blog-post 
description: writes great blog posts about research topics that the user wants, in Spanish
just_for:           # Add this!
   - blog-writer   # And this!
   - admin          # And this!
tool:     
  # ... the rest of your tool
```

The above means that a user with EITHER `blog-writer` OR `admin` roles will be able to call this tool. Other users will not.

## Indicating the user's roles in production

To indicate this you would make a request as follows (assuming your `runtime.yaml` says `client: sync`; check [how to get a session ID](./forma-agent-api-key.md#3-make-requests)):

```sh
curl -X POST -i http://localhost:8080/v1/chat \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id" \
  -H "X-Session-ID: $SESSION_ID" \
  -H "X-User-Roles: admin, another-role" \
  -H "Content-Type: application/json" \
  -d '{"role":"user","content":"write me a blog post, please!"}'
```

> **Note:** again, Forma will not validate neither the user ID nor the roles. You should have a separate service do this before calling Forma.

## Impersonating roles during development

When running `forma serve`, you are emulating a production server and thus you should pass the roles using the `X-User-Roles` header, just like in the example above.

The chat UI opened by `forma dev` has a **Roles** field at the top. Write the roles you want to have, separated by commas (e.g., `blog-writer, admin`), and they will be sent in the `X-User-Roles` header of your next messages. The field suggests the roles your agent's `just_for` lists mention, and remembers what you wrote when you reload the page. Leave it empty to be a user with no roles, for whom **restricted tools will not be available**.
