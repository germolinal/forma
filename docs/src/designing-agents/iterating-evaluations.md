# Iterating your evaluations

[Evaluating agent behaviour](./evaluating-agent-behaviour.md) lists *what* to evaluate. This chapter is about *how the evaluation itself gets built*: not in one go before the agent exists, but in a loop, alongside the agent. Like [Lessons from practice](./lessons-from-practice.md), it comes mostly from experience building agents, backed by a couple of published observations.

The short version: **don't set your datasets and metrics a priori. Write a first draft, run it, read every result, and fix whatever turned out to be wrong: the rubric, the dataset, or the agent.** Only once you trust the measurement do you start optimising the agent against it.

## Why a fixed evaluation fails

A dataset and a set of rubrics written before the first run look complete, and they never are. In my experience they fail for three reasons:

1. **Users ask what you didn't think of.** Don't be tempted to say "but who would ask this?". They will, and the agent has to deal with it, unless users can only pick from a predefined list. Questions you never wrote are questions you never tested.
2. **Rubrics have gaps you only see in real answers.** What should the judge do when two sources contradict each other? When a user asks a legitimate question in rude language? When an answer contradicts nothing in the sources but leaves out half of what matters? Until an answer like that appears, the rubric doesn't say, and the judge improvises.
3. **You change your mind about what "right" is.** Reading actual answers, you realise some questions should be answered differently from how you first thought: declined instead of answered, answered briefly instead of in full, referred to someone else.

The third one is not a failure of planning. Shankar et al. (2024) observed it in people grading LLM outputs and called it **criteria drift**: "users need criteria to grade outputs, but grading outputs helps users define criteria". You cannot fully write down what "good" means before you have seen what the agent actually does. Plan for it.

## The loop

### 1. Draft metrics for the agent's purpose

Start from the agent's definition (see [Defining your agent](./defining-your-agent.md)) and the [battery](./evaluating-agent-behaviour.md#the-battery), and keep what fits. Each metric scores **one property** (correctness, scope, tone…), with a short ladder of scores where every rung is described. Remember that a judge sees only the question, the answer and, optionally, a reference answer: anything it needs to know (the policy, the facts) must be in the rubric or the reference.

Treat this list as a draft. You will rewrite, split, merge and delete metrics in the next steps.

### 2. Generate random-ish data

Write a few questions by hand, and then let simulated users write many more: one per [persona](./user-personas.md) that can actually reach your agent (the novice, the hurried, the frustrated, the bad actor, the one who doesn't know what the agent is for…). Their job is breadth: questions you would not have written yourself. Some will look out of scope. Keep them: users can write anything.

Then read them. Generated questions repeat themselves, drift into the same phrasing, or come out malformed. Remove the noise, and add what is missing, including whole conversations for what a first message can't test (pressure, preferences, retention).

Simulated users don't write reference answers. Where a metric needs one, you have to produce it from the real sources.

### 3. Run, and read everything

Run the agent on the whole set, without letting failures stop the run, and **read each question, answer, score and explanation**. Not the mean: the individual cases. Every case where you disagree with the judge falls into one of three kinds, and each has a different fix:

| What you see | What is wrong | Fix |
| --- | --- | --- |
| A good answer with a bad score (or a bad answer with a good one) | The **rubric** | Rewrite the rubric so it scores the behaviour you want. Add the case as a worked example if the judge keeps getting it wrong. |
| A good answer, scored correctly by its rubric, but by the wrong rubric | The **dataset** | Move the question to the dataset whose metrics apply (e.g., from "answer with sources" to "decline politely"). |
| A bad answer, and the score says so | The **agent** | Note it, but don't fix it yet (see step 5). |

Signs that the evaluation, not the agent, is at fault:

- The judge's explanation contradicts its score.
- Two answers that behave the same get different scores.
- An answer that says nothing useful ("I can't help with that") earns partial credit.
- A style metric starts judging correctness, or a grounding metric punishes an incomplete answer as an invented one.
- Scores change a lot between runs of the same agent. Run each case several times before blaming anything.

### 4. Reorganise datasets by expected behaviour

Over a few rounds, datasets stop being "questions from persona X" and become "questions that should be handled in way Y": answered with references, answered without them, declined politely, refused, escalated to a human. Each dataset is paired with the metrics that apply to it.

Get used to moving questions between datasets. A question generated as a "valid question about topic X" may turn out to be one the agent should not answer; it belongs with the declines, scored by the "declines politely" metric, not with the answers, scored by correctness.

### 5. Freeze a baseline, then improve the agent

Stop iterating the evaluation when the rubrics score as you would, and every question sits in the right dataset. Not perfectly: *for now*. Run the current agent once more, unchanged, and keep that run as the **baseline**. Only now do you start changing prompts, tools and models, one at a time, comparing each run with the baseline.

The reason for the order: if you change the agent and the measurement together, you cannot tell which one moved the score. And an agent tuned against a broken rubric learns to please the rubric, not the user.

## The evaluation keeps changing

Freezing a baseline doesn't freeze the evaluation forever. It keeps changing, more slowly:

- **A failure found by hand, in review or in production becomes a case.** Add it to the dataset where it belongs.
- **A new kind of question may need a new metric**, or a new dataset.
- **When you fix a rubric, re-measure.** Run the *unchanged* agent with the new rubric, and use that as the new reference. The comparison with older runs is broken from that point on; say so in the run's name or notes. Such a run also measures noise: anything that moved without the agent changing was the rubric or chance.
- **Never change the agent to satisfy a measurement you don't trust.** If a metric can't be applied reliably, fix the rubric, fix the data, or use a stronger judge.

## Checklist

- [ ] Metrics are drafts until a run has been read in full; each scores one property.
- [ ] The dataset includes questions from simulated users, reviewed by hand, not only questions you wrote.
- [ ] Every disagreement with the judge has been classified: rubric, dataset or agent.
- [ ] Datasets are organised by the expected behaviour, each with the metrics that apply.
- [ ] A baseline was measured before the first change to the agent.
- [ ] After a rubric change, the unchanged agent was re-measured.
- [ ] Failures found by people become new cases.

## References

- Shankar, S., Zamfirescu-Pereira, J. D., Hartmann, B., Parameswaran, A. G., & Arawjo, I. (2024). [Who Validates the Validators? Aligning LLM-Assisted Evaluation of LLM Outputs with Human Preferences](https://arxiv.org/abs/2404.12272). arXiv:2404.12272. Criteria drift: defining criteria and grading outputs depend on each other.
- Husain, H. [Your AI Product Needs Evals](https://hamel.dev/blog/posts/evals/). Looking at your data, and evaluations that evolve with the product.
- Husain, H. [Creating a LLM-as-a-Judge That Drives Business Results](https://hamel.dev/blog/posts/llm-judge/). Calibrating a judge against a domain expert's critiques, iteratively.

> **If you use Forma:** rubrics live in `evals/metrics`, personas in `evals/personas` (run with `forma tester`), and datasets in `evals/data`, where an `input` can be a whole conversation. `forma eval --no-fail` runs everything without stopping on failures, and `-e` names each run. [Writing metrics and datasets](../how-to/metrics-and-datasets.md) and [Unit evals](../how-to/unit-evals.md) cover the details, and the `agent-dev-guidelines.md` that `forma init` creates describes this loop step by step for an AI assistant.
