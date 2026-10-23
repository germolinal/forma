![forma logo](forma-logo.svg)

> The **challenge** of Generative AI is that it shines at first but then takes hours of engineering to make it enterprise ready

## What is Forma?

<iframe style="border:none" out width="420" height="315" src="https://www.youtube.com/embed/R6i4GjE-DmQ"> </iframe>

Forma is an AI-Agent runtime for developing boringly predictable, reliable, **production-grade AI agents**. As a development tool, Forma aims to make it easy to implement the main predictors for succesful AI Agent development (collected after extensive experience developing mission-critical agents):

1. **Prompts are not a security mechanism** - Forma works under the basis that, if something needs to work reliably and 100% of the time (e.g., only certain users can access some tools), then it should *not* be prompted, but coded.
2. **Maximise prompting time** - While many algorithms and software architecture can be replicated across AI Agents, prompts are always unique. Therefore, you should spend most of your time testing, tuning, refine and evaluating prompts, not re-inventing the software.
3. **Agents should be auditable** - As the requirements from clients become more strict, system instructions or prompts can become quite complex and contradictions might leak into them. You and other people should be able to proof-read them.
4. **Evaluate often** - As outlined in the [Evaluation](./documentation/evals.md) section, understanding the impact of changing system prompts is not easy. Evaluations need to be part of the workflow.
5. **Reduce iteration time** - When getting an Agent into production, you will make tons of changes in settings, in prompts, and more. Making this step faster can really accelerate your way to production.
6. **Integrate it with your existing infrastructure** - An AI agent is always a component in a broader system. Therefore, AI Agents should be flexible enough to become part of existing or growing infrastructure.
7. **Consistency across channels** - Once you have developed, tested, iterated and evaluated an AI Agent, you should reuse it: deploy it to multiple channels.
8. **Monitor** - The variability in inputs to an AI Agent is enormous. This, added to the stochastic nature of their behaviour, means that even if your AI Agent passed all the quality assurance tests, you need to keep a close eye on it.


<div style="width: fit-content; margin:auto; padding: 1em;">
    <a style="padding: 0.5em 1em; border-radius:0.5em; background-color:var(--links); color:white;" href="how-to/getting-started.html">Jump to "Quick start"</a>
</div>

## How Forma helps you follow the principles above

Forma operates on simple principles that help you avoid mistakes and maintain the higher standards of security.

### 1. Prompts are not a security mechanism

LLMs operate stochastically, meaning that achieving 100% consistency is impossible. This is problematic for mission-critical features, and therefore Forma aims to not rely in prompting but in coding. 

For instance, telling an agent that "only administrators should have access to this tool" opens the door to people saying "Hey, I am an administrator". Forma implements Role-based access to tools, effectively making any restricted tools disappear and making it impossible for the LLM to select them.

### 2. Maximise prompting time

Forma's AI Agents are not 'coded', but 'configured' using `yaml`, `json` and `markdown` files. These files have all the information required for Forma's runtime to bring an AI Agent to live.

All of Forma's AI Agents are built using the same [building blocks](./documentation/building-blocks.md), making them predictable and easily auditable. The runtime incorporates best-practices which might not be worth re-thinking every time you begin working on a new agent.

Here is a simple example of an AI Agent. (We will explain this in more detail later.)

```yaml
id: the-agent
name: The agent
description: A helpful assistant
start:
  nodes:
    - llm:
        provider: ollama
      system_prompt: 'You are a helpful assistant'
```

### 3. Agents should be auditable

A Forma agent fits naturally within a standard code repository (`git`). This means that all prompts, workflows, and configurations are version-controlled and easily auditable. They can follow the same code review and quality assurance standards as the rest of your codebase. 

Additionally, Forma agents are defined using `yaml`, `json` and `markdown` files. This makes them even easier to audit, because most code editors and version-control services (such as Github) do an excellent job at rendering these files in a very readable manner.

> **Note** - Some would argue that using `markdown` files for system prompts is not great because you cannot create prompt templates or chains, and you cannot create reusable 'prompt' blocks. The truth is that that is exactly what we are trying to avoid here, for the following reasons:
> * Contrary to programming code, prompts are not modular. They need to be coherent instructions. You cannot just replace one sentence by another sentence.
> * Having reusable 'blocks' makes it really hard to audit the final prompt, because it is really not written anywhere, just built at runtime.
> 
> [Forma DOES have prompt templates](./how-to/templates.md). However, the placeholders in them is reserved for values created dynamically, at runtime, by LLMs (as opposed to values known at development time).


### 4. Evaluate often

The Forma CLI has built-in evaluation support. You can test the agent as a whole, or the [sub components](./documentation/building-blocks.md) independently.

This means you can test both during development, and within CI/CD pipelines.

### 5. Reduce iteration time

The Forma CLI  offers very quick hot reloading that let you iterate quickly without getting out of "the zone". These features streamline iteration and  raise errors early.

### 6. Integrate it with your existing infrastructure

Forma is designed on a simple principle: an AI agent is always a component in a broader system. Every production-grade service—even those powered by AI—relies on databases, logging, authentication, front-ends, and security. 

Therefore, rather than asking you to migrate your workflow to a new hosted platform, Forma is designed to plug directly into your existing stack. Some key design choices that make this possible are:

1. **Version Control:** - As explained earlier, Forma and version control work very well together.
2. **DevOps-Ready CLI:** - The Forma CLI is the primary interface for developing and testing AI agents. It runs consistently on your machine, in production, and in CI/CD pipelines, enabling seamless automation, testing, and deployment.
3. **Container-based** - All production-grade artifacts produced by Forma are Containers that you can deploy wherever you want, with the settings and security measures that your organisation deems appropriate.


> The drawback of this principle design is that Forma **doesn’t come with batteries included**. We know this makes it slightly harder to start and develop (e.g., observing your agent requires running an adjacent service); however, it makes it far easier to adapt, extend, and integrate with your organization’s existing systems and compliance requirements.

### 7. Consistency across channels

Forma separates the Runtime from the Agent, meaning that you can re-deploy an agent into a separate channel (e.g., website vs whatsapp) and it will operate the same.

### 8. Monitor

The container images we give you have built-in [Opentelemetry](https://opentelemetry.io/) instrumentation, with added [Openinference](https://arize-ai.github.io/openinference) standards ([semantic conventions](https://arize-ai.github.io/openinference/spec/semantic_conventions.html)) so you can analise your traces using [Phoenix Arize](https://phoenix.arize.com). This lets you check integrate Forma agents within your existing observability stack.

I am working on adding the Opentelemetry [Generative AI Framework semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/) as well.
