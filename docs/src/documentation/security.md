# Security Model

Our view is that the security aspects of an AI Agent deployed in the cloud can be classified into two categories:

1. **Cyber security** - This relates to "who" can ask a question to your agent, and how. Think of Firewalls, Authentication, Tokens, and more.
2. **LLM Security** - This relates to *what hapens when a person gets access to your agent*, and how well will your agent handle the infinite amount of potential messages, prompts or inputs that a person can send. While it follow its original instructions? or can it be tricked?

The Forma team has two very different positions on these.

## Cyber security

This is essential, and **we aim to make it easy for you to add as much security as you want/need to our agents**. 

That siad, it is not our focus or responsibility and it is ultimately **your responsibility to protect the Forma Agents** with the appropriate level of security for your needs and regulations.

Forma provides a few strong foundations, but leaves architectural control to you:

1. **Safe by design** - Forma is developed using programming languages that enforce strict type systems, memory safey, and concurency guarantees.
2. **Minimum runtime** - Forma Agents can run inside [Distroless Containers](https://github.com/GoogleContainerTools/distroless), meaning that you can restrict the runtime container to just what is necesary to run the Agent. No package managers, shells, or other standard Linux programs.
3. **Opt-out API Key enforcement** - Forma Agents require Api keys, which you define. (You can explicitly opt-out of this during development.)
4. **Encrypted credentials** - Credentials stored by Forma (e.g., the tokens used to connect to MCP servers) are encrypted using a key you provide (`FORMA_ENCRYPTION_KEY`).

## LLM Security

Contrary to Cyber security, LLM Security is our main focus, but absolute security is impossible. There are two main reasons for this. First, the inputs that people can send to an LLM are just too broad (they can literally write whatever they want, or send *any* image); and second, LLMs are all different and stochastic, therefore, if you let you choose the LLM you want to use, we cannot guarantee it sill perform well.

However, this does not mean you are unprotected. Forma offers:

1. **Auditability** - Forma AI Agents are easily auditable
2. **Evaluations** - Forma makes it dead easy for you to evaluate your agents before deploying
3. **Rapid iteration** - Forma allows for rapid iteration and prompting, and avoids coding. This means that you spend most of your time improving your agent, not waiting or programming well-known solutions.
4. **Monitoring** - By being Opentelemetry-compatible, Forma Agents can be monitored in production
5. **Role-based access** - Tools can be set to require explicit roles (e.g., only managers can write to a database). Check the [Role Based Tool Access](../how-to/role-based-tools.md) section.
6. **Human in the loop** - Tools can be set to require confirmation before being called (`confirm_before_calling`). This is enforced in code: the session is suspended until the client confirms or rejects the call.

