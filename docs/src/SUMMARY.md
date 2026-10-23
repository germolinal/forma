# Summary

[What is `Forma`?](./intro.md)

---

# Conceptual guides

- [Components of a Forma Agent](./documentation/building-blocks.md)
- [How a request is processed](./documentation/request-lifecycle.md)
- [About an AI Agent's memory](./documentation/memory.md)
- [Forma Philosophy (Prompting over Coding)]()
- [Security Model](./documentation/security.md)
- [Architecture Overview](./documentation/architecture.md)
- [Evaluations](./documentation/evals.md)
- [Client flavours]()

---

# Learning about LLMs

- [How an LLM sees a conversation](./learn/conversations.md)
- [Tokens, context windows and cost](./learn/tokens.md)
- [What tool calling really is](./learn/tool-calling.md)

---

# Designing AI Agents

- [Designing AI agents](./designing-agents/index.md)
  - [Defining your agent](./designing-agents/defining-your-agent.md)
  - [User personas](./designing-agents/user-personas.md)
  - [Structuring responses](./designing-agents/response-structure.md)
  - [Tone and personality](./designing-agents/tone.md)
  - [Questions at the end](./designing-agents/closing-questions.md)
  - [Small talk and scope](./designing-agents/small-talk-and-scope.md)
  - [Errors, uncertainty and trust](./designing-agents/trust-errors-uncertainty.md)
  - [Bad actors and security](./designing-agents/security-and-bad-actors.md)
  - [Evaluating agent behaviour](./designing-agents/evaluating-agent-behaviour.md)
  - [Checklist and prompt template](./designing-agents/checklist-and-prompt-template.md)
  - [Lessons from practice](./designing-agents/lessons-from-practice.md)

---

# Hands-on Guides

- [Getting started](./how-to/getting-started.md)
  - [Development Environment]()
  - [Dory, your first Forma agent](./how-to/dory.md)
  - [Dory + contextual memory](./how-to/contextual-memory.md)
  - [*What* is Dory doing? (Traces)](./how-to/dory-traces.md)
  - [*How* is Dory doing? (Evals)](./how-to/evaluations.md)
  - [Workflows](./how-to/workflows.md)
  - [Templates](./how-to/templates.md)
  - [Tools](./how-to/tools-introduction.md)
  - [Clients and User Interfaces]()
  - [Configuration]()
- [Diving deeper](./how-to/diving-deeper.md)
  - [Role Based Tool Access](./how-to/role-based-tools.md)
  - [Human in the loop](./how-to/human-in-the-loop.md)
  - [More on Evals](./how-to/unit-evals.md)
  - [Writing metrics and datasets](./how-to/metrics-and-datasets.md)
- [Authentication]()
  - [Managing Secrets]()
  - [Forma Agents' API Key](./how-to/forma-agent-api-key.md)  
  - [Authenticating to LLM Providers](./how-to/llm-auth.md)
- [Deployment](./how-to/deployment.md)
  - [Deploying to Google Cloud](./how-to/cloud-deployment.md)
  - [HTTP API](./how-to/http-api.md)
  - [Your agent on WhatsApp](./how-to/whatsapp.md)
  - [Your agent on Telegram](./how-to/telegram.md)
  - [Building Forma from source](./how-to/building-forma.md)
  - [Observability and Monitoring](./how-to/observability.md)
  - [Best Practices for Enterprise Use]()
  - [Continuous evaluation]()

---

# Detailed Examples

- [AI Agents](./examples/agents/example-agents-intro.md)
  
- [Client applications](./examples/clients/clients-intro.md)
  - [Vercel AI-SDK v5 - Part 1](./examples/clients/vercel-aisdk-5-setup.md)
  - [Vercel AI-SDK v5 - Part 2](./examples/clients/vercel-aisdk-5-persist.md)
---

# Help

- [Troubleshooting](./help/troubleshooting.md)
- [Glossary](./help/glossary.md)

---

# Reference Documentation

- [Agent](./reference_docs/auto-agent.md)
- [Eval](./reference_docs/auto-eval.md)
- [FormaRuntime](./reference_docs/auto-formaruntime.md)
	- [Client](./reference_docs/auto-client.md)
		- [OpenAISyncAdapter](./reference_docs/auto-openaisyncadapter.md)
		- [OpenAISSEAdapter](./reference_docs/auto-openaisseadapter.md)
		- [AiSdkV5Adapter](./reference_docs/auto-aisdkv5adapter.md)
		- [WhatsappAdapter](./reference_docs/auto-whatsappadapter.md)
		- [TelegramAdapter](./reference_docs/auto-telegramadapter.md)
	- [Storage](./reference_docs/auto-storage.md)
	- [Workspaces](./reference_docs/auto-workspaces.md)
		- [GcsWorkspaces](./reference_docs/auto-gcsworkspaces.md)
		- [LocalWorkspaces](./reference_docs/auto-localworkspaces.md)
	- [Sandbox](./reference_docs/auto-sandbox.md)
		- [CloudRunJob](./reference_docs/auto-cloudrunjob.md)
		- [LocalDocker](./reference_docs/auto-localdocker.md)
- [GenaiClient](./reference_docs/auto-genaiclient.md)
	- [Ollama](./reference_docs/auto-ollama.md)
	- [OpenaiV1](./reference_docs/auto-openaiv1.md)
	- [Groq](./reference_docs/auto-groq.md)
	- [VertexAI](./reference_docs/auto-vertexai.md)
- [InputVariable](./reference_docs/auto-inputvariable.md)
- [Metric](./reference_docs/auto-metric.md)
- [Node](./reference_docs/auto-node.md)
- [Tool](./reference_docs/auto-tool.md)
- [ToolRunner](./reference_docs/auto-toolrunner.md)
	- [TemplateTool](./reference_docs/auto-templatetool.md)
	- [WorkflowTool](./reference_docs/auto-workflowtool.md)
	- [ApiCall](./reference_docs/auto-apicall.md)
	- [DelegatedTool](./reference_docs/auto-delegatedtool.md)
	- [McpTool](./reference_docs/auto-mcptool.md)
	- [MailgunTool](./reference_docs/auto-mailguntool.md)
	- [TavilyWebSearch](./reference_docs/auto-tavilywebsearch.md)
	- [GoogleAgentSearch](./reference_docs/auto-googleagentsearch.md)
	- [GoogleGenImage](./reference_docs/auto-googlegenimage.md)
	- [FetchTool](./reference_docs/auto-fetchtool.md)
	- [GoogleWorkspace](./reference_docs/auto-googleworkspace.md)
	- [Scrapbook](./reference_docs/auto-scrapbook.md)
	- [CodeExecution](./reference_docs/auto-codeexecution.md)
	- [CurrentDateTool](./reference_docs/auto-currentdatetool.md)
- [Workflow](./reference_docs/auto-workflow.md)
