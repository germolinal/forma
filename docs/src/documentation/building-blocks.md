# Core Concepts

Forma is built around a small set of concepts. By understanding these, you can read, write, and reason about any agent you define. Having a small set of concepts also helps understand traces and debug information better.

## 1. LLMs

The lowest level of an Archetype agent is the Large Language Model (LLM) client, which we just call 'LLM'. This is a small machine that sends a set of messages to an LLM provider (e.g., Claude, Gemini, OpenAI or Ollama) in order to produce a response.

Different providers (OpenAI, Anthropic, Ollama, etc.) can be swapped in and configured without changing your agent’s logic.

## 2. Nodes

A **Node** is the basic building block of a workflow. Each node always follows the same steps:

1. **Triage** – an LLM reads the whole conversation and decides what to do next: reply right away, or call tools.
2. **Tools (optional)** – the node runs the tools the LLM chose, and triage runs again with their results. This repeats until triage replies without calling a tool, and that reply is the node's answer, or until `max_iterations` rounds of tools have run.
3. **Summarisation (only at the limit)** – if `max_iterations` runs out while the LLM is still calling tools, the node calls an LLM once more, without tools. It is told it reached the limit, so it says what it found and what is missing, and asks the user, in the language of the conversation, whether to go on.

Key points to remember:

- Nodes _always_ triage.
- Tools may or may not be executed. Specifically, there is no guarantee that a specific tool will be ran and thus it is important that its output is not required downstream.
- Summarisation only runs when `max_iterations` is reached; otherwise, the last triage is the answer.
- Both Triage and Summarisation have access to the **entire context** (conversation history, prior tool outputs, etc.).

## 3. Workflows

**Workflows** are a mechanism to break down large tasks into smaller—more focused—tasks. This is beneficial because complicated tasks—which require very large system prompts—and thus the AI Models will struggle to follow those instructions faithfully. By breaking down a big task into smaller bits, you can provide more precise, prevent contadictions in your prompts. Like people, LLMs perform better with clear and focused instructions.

A **Workflow** is a set of nodes that depend on each other (for the Geeks, it is a Directed Acyclic Graph of nodes). 

> **Example**
> 
> We can break down this: 
> 
> ```
> Take this academic article and produce a blog post for 5th-graders, 
> in Spanish. 
>
> The content of the blog post should include a brief 
> introduction/motivation, a brief explanation of the methodology,
> and an emphasis in the results and implications.
>  
> Your target audience is 5th graders, so do not use acronyms or jargon.
> Use examples to make it more relatable.
> ```
> 
> Into these more focused steps
> 
> 1. `Create a list containing the (1) motivation; (2) methodology; (3) results and implications from the following paper` 
> 2. `Write a blog post for 5th graders based on the following summary points. Explain the motivation, outline the methodology and emphasise the results and implications. Use examples to make it more relatable.` 
> 3. `Translate this blog post into Spanish. Keep the tone and length of the post.`


An interesting feature is that a workflow can itself be exposed as a **tool**. This means that a node can decide to call a completely different worflow and then use its output to answer a question. For instance, the example above—the workflow that writes blog posts based on academic papers—could be a tool within a larger AI Agent that cannot only do that, but also other tasks (e.g., write abstracts or format references). This makes workflows both composable and reusable, and let Archetype agents implement complex logic at scale.


## 4. Agents

An agent is a wrapper of a root Workflow and (potentially, in the future) a set of extra callbacks. 

## 5. Runtime

Separate from the agent, there are Runtime settings (by convention, stored in a `runtime.yaml` file at the root of your project). It contains information about the client application (e.g., what kind of messages will it send, and what it expects to receive), whether an API key is required, where conversations are kept, where the agent's workspaces are, where its code runs, and more. Check settings in [the docs](../reference_docs/auto-formaruntime.md).

> **Note**: The runtime settings are used by `forma serve` and `forma dev` during development (the latter ignores the client, as it serves its own chat UI), and by production servers, as explained in the [Deployment](../how-to/deployment.md) section.

