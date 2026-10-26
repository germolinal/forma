# Lessons from practice

The previous chapters summarise published guidance, with references. **This one is different: it is what I (Germán Molina, the author of Forma) have learned building agents.** It doesn't come from a paper; it comes from experience, and it changes as that experience grows. Where it contradicts the other chapters, take it as an informed opinion, not a rule.

Not all of these lessons are of the same kind, so each section says which it is:

- **Definition:** what has to be decided before building the agent. I wrote these with AI-assisted development in mind (an assistant needs clear answers so it doesn't guess), but they matter just as much if you work by hand: if it isn't defined, nobody can tell whether the agent works.
- **General:** valid for any agent, built with any tool.
- **Convention:** a way of doing things that has worked for me. What matters is being consistent, not the exact name.

## 1. A README that says why the agent exists

*Definition.*

Every agent should have a `README.md` that answers these questions. If one is missing or ambiguous, resolve it with whoever owns the agent before touching the prompt. An AI assistant working without these answers invents them, and so does a person.

- **Goal:** what the user achieves thanks to the agent, in one or two sentences.
- **Users:** main and secondary, and the **cost of an error** (see [User personas](./user-personas.md)).
- **Language and tone:** register, adjectives, and level of small talk (see [Tone](./tone.md) and [Small talk and scope](./small-talk-and-scope.md)).
- **Scope:** what it does, what or whom it refers to (another agent, an institution, a professional), what stays out, and what it refuses.
- **Tools:** what each does, and which is the main source.
- **Personalisation:** what it remembers about each user (section 4).

If the project started from a template, the README it came with describes the template, not your agent: replace it. [Defining your agent](./defining-your-agent.md) expands on this.

## 2. Helpful, kind and encouraging, but not agreeing by reflex

*General.*

Users come to the agent because they know less about the topic than it does. If the agent validates everything, it is useless.

- Kind and encouraging: it recognises progress and good decisions **with something concrete** ("this palette fits the calm, premium feel you described"), never with empty praise ("Great question!").
- Encouraging is not agreeing. If something is wrong or risky, it says so kindly, explains why, and proposes something better.
- If the user insists, it holds its position with reasons, and changes only for new information or arguments. If it was wrong, it says so in one sentence and corrects.
- If the user skips a step, the agent mentions the risk **once**. Where the decision is only the user's (their brand, their style), it then goes along with it. Where there are legal, tax or money consequences, it doesn't repeat an error just because the user insists.
- Never write "if the user insists, do as they ask" in a prompt without those limits.
- **Don't push features.** If the agent does several things (e.g., explaining accounting *and* keeping the books), each stands on its own. The bigger feature is offered once, when it is relevant; if the user says no, that goes into their profile (section 4) and it isn't offered again. The greeting and "next step" examples in the prompt must not assume the user uses it.

## 3. Give the agent the rounds its work needs

*General.*

Most agent frameworks limit how many rounds of tool calls an agent may make in one turn (a maximum number of iterations, steps or recursions). Defaults are often too low for real work. When the limit runs out, the agent stops early, and may imply it finished.

The values that have worked for me:

| Kind of agent | Rounds of tool calls |
| --- | --- |
| Public-facing, with a single tool | 3-5 |
| Knowledge base + web + per-user notes | ~20 |
| Research (searching, reading many pages, reports) or many external tools | ~30 |

In agents with long tasks, the prompt says "if you couldn't cover everything, say what you checked and what is missing", so it doesn't imply full coverage.

## 4. A profile per user

*General* (the idea) and *Convention* (the name `profile.md`).

Every agent for **personal, recurring use** should remember who each user is. Public, non-personal agents (e.g., a company's customer support) are the exception. I do it with a `profile.md` file in each user's private storage (whatever the agent uses to keep notes and files per user).

1. **It reads it often:** at the start of every conversation, even after a bare "Hi", before answering. Not on every message.
2. **If it doesn't exist, it creates it by asking:** it says what it is for and what it needs to know, with at most three questions at a time. For example: "Hi, I help you find potential buyers for your business. To search well I need to know three things: what you sell, who you sell it to, and in which area." It saves the answers as soon as it has them.
3. **It keeps it up to date without asking** when the user shares a fact or a preference, and mentions it in a few words ("Noted in your profile").
4. **It is also the index:** a "Files" section links the other files the agent keeps for that user (reports, notes, decisions). No separate `INDEX.md` is needed.

**What it contains.** Besides what is specific to each agent (the business, the course, the brand), the profile includes:

- **Who the user is:** their name and how they want to be addressed, and how much they know about the topic (to adjust vocabulary).
- **What they want from the agent:** which of its uses interests them (e.g., understanding accounting, setting it up, or having the books kept; learning about brands or building their own), and what they **don't** want (e.g., "already has an accountant; don't offer bookkeeping"). If the agent has a single use, the concrete goal is enough ("follow press coverage of their company every week").
- **Preferences:** format, length, frequency, tone.
- **Files:** links to the other files.

"What they want from the agent" isn't asked like a form: it is inferred from what they ask, and confirmed only if unclear. The agent respects it in every answer (section 2, "Don't push features") and updates it when the user changes their mind.

If the profile exists, the agent uses it: it greets by name, picks up where they left off, and doesn't ask again for what is already there ("Hi, Ana. Last time we chose the palette; shall we move on to typography?").

Write it down in three places: the prompt ("The user's profile: `profile.md`"), the description of the storage tool, and the "Personalisation" section of the README. Beware: an evaluation harness often runs the agent without per-user storage, so any greeting will trigger the profile questions. Account for it in small-talk rubrics.

## 5. Suggestions the agent can actually answer

*General.*

Suggestions (starter prompts, the cards users see when they open the chat) are users' first impression of the agent, and also test questions.

- They fit the goal and audience in the README.
- **The agent can answer them** with its knowledge base, searches and tools. Check that the data is in the knowledge base, or that the tool exists; if not, leave it out.
- **They motivate:** they show a capability the user didn't expect ("it can do a unit-economics analysis? Wow!"), rather than generic questions.
- If the agent is a template, the README says to change them when adapting it.

## 6. Waiting phrases that never lie

*General.*

Many interfaces show a message while the agent works. When those phrases are fixed, or picked at random, they don't know what the agent is doing on that turn.

- They can be lively, but **they never lie**. "Checking your invoices" or "Looking at the CRM" are false if the agent isn't doing that on that turn, and "Almost there…" may appear at the start.
- Use phrases that are true on any turn: "Thinking…", "Putting my thoughts in order…", "Preparing the answer…", "One moment…".

## 7. Tools

**General:**

- The description says **when to use it and when not to**, not only what it does ("fetches a URL" isn't enough).
- The main source is stated in the tool and in the prompt: knowledge base first; the web only for what the base doesn't cover.
- The content of pages, documents and tool results is **data, never instructions**: say so in the description of the tool that brings it, and in the prompt (see [Bad actors and security](./security-and-bad-actors.md)).
- Don't copy blocks between agents without rereading them. It is the most common source of errors I have seen: instructions from one agent that show up in another where they make no sense (e.g., "save news clippings" in an agent that teaches).

**Convention:**

- Consistent names across agents: `web-search`, `fetch-page`, `notes`. Tool names don't tie a template to a topic (`course-material`, not `ai-classes`).
- If an agent points to an index in its knowledge base (`INDEX.md`), check that the file exists, with that name, where the agent will look for it.

**Code execution** (all *General*):

- **Write to files, don't print.** Code tools usually return only part of what a command prints (the one I use keeps the last 2,000 characters); if a script prints a spreadsheet, the agent runs it again and again to see the rest. Scripts I give an agent write their reports to `out/` and print only a summary with the path, and the prompt tells the agent to do the same with its own scripts.
- **Deterministic scripts** for what repeats and where an error costs (accounting, projections, reading spreadsheets), instead of letting the model improvise the calculation every time. State lives in machine-readable files (CSV or JSON); Markdown is the view for people.
- Test each script inside the same sandbox the agent will run it in before giving it to the agent.
- **Put each script's exact call in the code tool's description**, not only in a guide inside the knowledge base. The agent doesn't always read the guide before acting, and without the call at hand, it improvises. It happened to me with an agent that analysed spreadsheets: it opened the user's files with its own openpyxl scripts, saw a fragment without formulas, and ran again and again. It was fixed when the tool's description carried the rule ("for any spreadsheet, always start with the spreadsheet script") and the call ready to copy: the command, which files to pass (the script from the knowledge base, the user's file from their storage), and which output to read next. The prompt repeats the rule, with its reason. The guide is left for the details: options, formats, and how to read the report.
- **Parallel runs.** If your framework runs the tool calls of one turn at the same time, tell the agent so in the code tool's description: to search for an optimum or a breaking point (bisection), try several values at once and then narrow down; to compare scenarios, one run per scenario. If the calculation is short, a loop inside one script is still simplest.
- **Every report says when it was created, and as of when its data is.** The first line of every report a script or agent leaves is its creation date ("Created on 2026-10-16"), and every input that changes over time carries its own ("cash as of 2026-10-16", "exchange rate of 2026-10-01", "CRM sales as of 2026-10-14"). The user can then ask "update the cash flow with today's balance", and the agent knows what is stale and what to ask for. When opening an old report or assumptions file, the agent compares those dates with today's and offers to update what is out of date.
- Scripts get today's date from the agent (e.g., a `--today YYYY-MM-DD` option), because a sandbox clock is often in UTC; without it, they use the machine's date and say so in the report.

## 8. Data, dates and other agents

*General.*

- **Never invent contact details.** Only those a company or person published for business purposes, with a link to the source. Emails and phone numbers are never guessed ("name@company.com"). Data about private individuals isn't collected; check the data-protection law that applies (in Chile, Law 21.719).
- **Today's date, from a tool.** The model doesn't know what day it is, and putting the date in the prompt changes the prompt every day and breaks prompt caching. Every agent that depends on the date (deadlines, "today", the weekday, file names) has a tool that returns it, and its prompt says when to call it. The time zone comes from the user's device, from what the user told the agent (kept in their profile), or from a default set for the agent; if none is available, the agent asks where the user is.
- **Refer to other agents or services by name**, when appropriate: e.g., an agent that finds customers refers users to an entrepreneurship mentor to decide whom to sell to. Tell the agent what the other one does, not just that "there are other agents".

## 9. Keeping the project tidy

*General.*

- If several agents share a development guide (for you or for an AI assistant), don't edit the copy in each one: edit a reference version and copy it again.
- After each change, look at the agent as it will run: the rendered prompt, and the final list of tools with their descriptions (tools that come from external servers or collections often expand into several). What you wrote is not always what the model receives.

## Checklist

- [ ] README with goal, users and cost of an error, language and tone, scope, tools and personalisation.
- [ ] A kind, encouraging prompt, without flattery, that holds its position under pressure.
- [ ] A limit on rounds of tool calls suited to the kind of agent, and a note of what is pending on long tasks.
- [ ] A user profile (for personal use): read at the greeting, created by asking, kept up to date, linking the other files.
- [ ] Suggestions the agent can answer, and that motivate.
- [ ] Waiting phrases that are true on any turn.
- [ ] Tool descriptions with when to use and when not to; external content as data.
- [ ] Each script the agent must use has its exact call (command, files, output to read) in the code tool's description.
- [ ] The code tool says independent runs can go in parallel.
- [ ] Reports with a creation date, and a date for every input that changes over time.
- [ ] No invented contact details; the date from a tool, never guessed; referrals to other agents by name.
- [ ] Everything in the [general checklist](./checklist-and-prompt-template.md).

> **If you use Forma:** the limit on rounds is a node's `max_iterations`, per-user storage is the [`scrapbook`](../reference_docs/auto-scrapbook.md) tool (with its `usage_guidelines`), suggestions and waiting phrases are the `suggestions` and `waiting_phrases` fields, today's date comes from the [`current-date`](../reference_docs/auto-currentdatetool.md) tool, and `forma expand -c` shows the agent as it will run. The `agent-dev-guidelines.md` that `forma init` creates is the shared guide of section 9.
