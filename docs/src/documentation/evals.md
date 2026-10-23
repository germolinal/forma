Generative AI is amazing for demos. It takes one prompt and 5 minutes to get an impressive chatbot running. And yet, it can then take ages to reach the **consistent level of quality** that will make you trust it enough to talk to your clients autonomously. **Evaluations** are the answer to this problem.

## What is an evaluation

In simple terms, an evaluation is actually a very simple concept:

1. You have a sample question
2. You ask that question to your agent
3. You then you use some metric or rubric to decide whether the answer is good enough or not, and why. 

> **Note**: Sometimes knowing whether the answer was "good enough" requires also having a "sample answer". For instance, knowing whether the agent responded in a factually correct manner implies knowing the real answer. On the contrary, checking whether the agent responded in a serious manner and without using emojies does not. We will talk about this later.

## Why are evaluations so crucial

Ensuring the quality of Generative AI Agents quickly becomes a challenge because of three main reasons:

1. Agents' answers change every time
2. Agents' answers are generally qualitative
3. Sometimes the quality of the answers is subjective (e.g., the same question can be answered "succesfully" in multiple ways)

These three issues affect both how you **build** your agent and how it behaves once **deployed**. Let’s look at them one by one.

### Answers change every time

Imagine that your company is developing a customer service agent. Initial tests indicate that your chatbot is worth keeping, but users' feedback indicate that it "should be more friendly".

Based on this feedback, the developers add a line to the system prompt, emphasizing the need to be "friendly". People on your team then test it, and notice that the answers have changed for the better. **The challenge during development** is that you cannot know whether this is a consistent change, caused by the new system prompt; or if it is caused by the fact that—as expected—these answers are not the same as before. 

Something similar happens **in production**: You cannot know how many of your users will perceive any change, for the better or worse.

### Agents' answers are generally qualitative

Computer scientists and mathematicians have dealt with random numbers for a while, so in many cases the problem of "answers changing every time" would not be an issue. This is not the case of answers based on language.

You see, when you have numbers you can run statistics. Therefore, you can reach conclusions such as "based on these answers we have so far, it is only 1% possible that a user will see a catastrophic error". In our case, you cannot calculate averages or standard deviations... or can we?

Yes we can, in the same way teachers can mark an essay and give the student a grade: using a **rubric** or a **metric**. Rubrics turn intuition into repeatable measurement. They make it possible to test generative systems scientifically, under the condition that the rubric itself captures what good really means for your use case.

Creating rubrics is sometimes easy (e.g., the answer to "how many moons does Earth have?" is 1). However, sometimes it is hard (e.g., there is not a single answer to "Good morning"... what if the answer has emojies 😀?). Rubrics are guidelines and criteria for  assigning a grade. For example "the answer should be factually correct to get a 1" or "One point for every work well spelled".

> **Note**: When doing this, **we are turning qualitative data into quantitative one**. An implication of this is that, while we can now calculate averages and standard deviations, these will not be too meaningful unless our transformation of qualitative data into quantitative one is good enough.

### The quality of an answer can be subjective

This is very related to the previous point: turning a qualitative answer into a number is not obvious. 

For example, imagine you ask your customer service agent to reply to “I’m upset because my order was delayed.” Depending on your brand and audience, there are multiple "good" answers:

* A formal company might prefer: “We sincerely apologize for the delay. We’re already investigating and will update you soon.”
* A friendly startup might go for: “Oh no! That’s on us — so sorry! Let’s fix this right away 💪.”
* A luxury brand might say: “We are truly sorry for the inconvenience, and we’ll make sure your next experience exceeds expectations.”

All of these responses can be considered “correct” — but they express different personalities, tones, and priorities. Whether they are good enough depends not only on correctness, but also on brand voice, customer expectations, and even the mood your company wants to convey. This is why evaluations can never be completely objective. They must reflect your brand values, tone, and audience expectations, not just correctness. The next step is to decide how you’ll measure that.

## What is a good answer?

Evaluating whether an answer is good or not can be done in several ways, depending on what you want to evaluate.

### Reference-based evaluations

These are evaluations that compare the new response produced by your agent with a pre-defined or expected response to the sample question. This can be done by assessing factual accuracy (e.g., "the answer should be exactly the same", or "one point for every planet correctly named"), or just semantic similarity (e.g., "the new answer should be semantically similar to the reference answer".

### Heuristic evaluations

Sometimes you do not need a reference answer. For instance, you—as a human—do not need a reference answer to know whether a response is "funny" or "sad", or if it seems like a valid email or not.

## Who will be the judge?

A lot of the time, you can ask a Large Language Model to provide a score:

```
You are an email proof-reader. You need to evaluate the 
following email :

"[insert email]"

Based on the following principles

1. Should be not longer than 2 paragraphs
2. Should have an appropriate UK english spelling
3. Should be polite

# Evaluation criteria

- Grade it with 0 points if it does not comply with any criteria
- Grade it with 1 point if it complies with 1 criteria
- Grade it with 2 point if it complies with 2 criteria
- Grade it with 3 point if it complies with all the criteria

```

Using an LLM as a judge is flexible and fast. It can evaluate hundreds of answers at once using your rubric. However, it introduces its own variability and bias. Deterministic metrics (like [ROUGE](https://en.wikipedia.org/wiki/ROUGE_(metric)) or [BLEU](https://en.wikipedia.org/wiki/BLEU)) are less nuanced, but more consistent. In practice, combining both gives the best balance between reliability and depth.


## Best practices when evaluating an AI agent

Our view is that this is still a developing area, but here are some of the best practices we have identified so far:

1. **Run evaluations very often during development** - Having a set of questions that you can run simultaneously will not only give you a better indication of whether your prompts are consistently improving the performance of the agent (e.g., "it is now correct 80% of the time"), but can also highlight patterns (e.g., "it is never saying Good Morning!").
2. **Run evaluations every time before deploying** - The changes made to the system prompt have improved the performance of the agent? Prove it. Run a test suite before deploying, every time. Compare them with what is now deployed.
3. **Iterate your rubrics/metrics along with your agent** - Turning qualitative answers into numbers is not a trivial task, and it requires iteration. When starting a new project with evaluations, you will notice quite quickly that some answers that are given a relatively bad score are actually very acceptable. When this happens, you might need to change your rubric (e.g., "... Also consider that if the user seems sad, the response should not be *funny*").
4. **Develop your rubrics and metrics with other team members** - "What is good" is a company decision, based on testing feedback and also communication guidelines. Decide what should be evaluated as a team.
5. **Keep your evaluations and rubrics focused** - As a general rule: the clearer the question, the better the answer. If you have a single evaluation that measures whether "the response sounds acceptable for a business context", then you are relying heavily on what the Evaluator considers "appropriate". If, on the contrary, you have multiple evaluations that break down what it means to be appropriate (e.g., never rude, not *cool*, no emojies, factually correct, etc), then your evaluations will be better.


Evaluations turn intuition into evidence. They allow teams to iterate confidently, prove improvements, and maintain consistency as agents evolve. Whether automated or manual, well-designed evaluations are what turn a good demo into a reliable product.

## How Forma helps

Forma makes it easy to bring evaluations into your development and deployment workflow from day one. Because agents in Forma are composed of nodes, workflows, and agents, you can evaluate the quality and consistency of each of these layers independently. This means you can test a single node’s decision logic (e.g., “does the summarizer respond politely?”), a workflow’s structure (e.g., “does this pipeline produce a coherent final output?”), or the full agent end-to-end — using the exact same tooling.

Evaluations can be executed directly from the CLI, which makes them easy to automate in CI/CD pipelines or DevOps environments. Each evaluation run produces structured results, written to your project as JSON and as a Markdown report you can read—or review in a pull request—without anything else running. They can also be pushed to Phoenix Arize instead, where you can visualize trends, compare experiments, and curate datasets for further training or tuning. This combination of modular testing, automation, and observability turns evaluations from a manual QA process into a continuous, data-driven feedback loop — helping you systematically raise your agent’s reliability and quality over time.