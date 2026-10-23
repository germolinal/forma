# More on Evals

The [intro to evaluations](./evaluations.md) covered the basics of evaluations in Forma. To summarise, it consists of:

1. Create a dataset (the `forma tester` command can help create it)
2. Put the dataset where Forma can find it—`./evals/data`—which is where `forma dataset-upload` copies it
3. Run evaluations using the `forma eval` command. 

There is more to evaluations than that, however. This section provides more details on Evaluations, including creation and maintenance of datasets.

## Where datasets and results are kept

Forma can keep datasets and evaluation results in two places, chosen with the `--platform` option (or the `FORMA_EVAL_PLATFORM` environment variable):

* **`file`**, the default. Datasets are JSON files in `./evals/data`, and results are written to `./evals/results`—as JSON, so they can be processed, and as Markdown, so they can be read. Nothing needs to be running, and the directory can be changed with `FORMA_EVAL_DIR`.
* **`phoenix`**. [Arize Phoenix](https://phoenix.arize.com/) is an open-source LLM tracing and evaluation project, which will store your datasets, let you edit and extend them, and visualise the results of every experiment. It has to be running—`docker-compose -f ./dev/docker-compose.yaml up` does that—and Forma looks for it at `PHOENIX_EVAL_ENDPOINT`.

> 📌 **Note** that the two do not talk to each other: a dataset uploaded to Phoenix is not on your filesystem, and vice versa. `forma dataset-download` can be used to move one from Phoenix into a file.

## Datasets

A dataset is no more than a set of prompts that will be used to test and assess the quality of an AI Agent.

Datasets have a name, a description, and the data with which to test. These can be created manually, or with the help of the `forma tester` command. This is the short user manual:

```text
Usage: forma tester [OPTIONS] --file <FILE>

Options:
  -p, --path <PATH>                The path where the 'personas' descriptions are kept [default: ./evals/personas]
  -f, --file <FILE>                The name of the 'persona' file within the given path
  -o, --output-path <OUTPUT_PATH>  The relative path of the output dataset file [default: ./evals/data]
  -n, --n <N>                      The number of questions to try [default: 5]
  -a, --action <ACTION>            Whether to 'append' to an existing dataset file or 'create' (overwrite) it [default: create]
  -h, --help                       Print help
```

> 📌 **Note** that personas tend to repeat themselves, especially on small local models. Each new question is generated knowing the previous ones, which helps, but you should still review the dataset by hand. Giving the persona a concrete list of topics to choose from also helps.

For instance, let's examine the dataset we created in the intro to evaluations, using the command 

```sh
forma tester -f wanderer.yaml -n 5
```

This generates 5 datapoints using the *wanderer* persona. The file is in the default location (`./evals/personas`) and the output file will go in the default destination (`./evals/data`). The file looks like this:

```json
{
  "name": "wanderer",
  "description": "A dataset by Persona 'wanderer'",
  "data": [
    {
      "input": "What is the purpose of this conversation?\n"
    },
    {
      "input": "Who created this site?\n"
    },
    {
      "input": "Is this AI being monitored or is it generating responses independently of human oversight?\n"
    },
    {
      "input": "What's the purpose of this site? Is it some kind of game or a real person behind this?\n"
    },
    {
      "input": "Who created this website?\n"
    }
  ]
}
```

Because it was written to `./evals/data`, this dataset is ready to be used. A dataset that lives anywhere else—or that should be kept in Phoenix—can be put in place with `forma dataset-upload`:

```sh
forma dataset-upload -f ./evals/data/wanderer.json --platform phoenix
```

That command can also *add* to a dataset that already exists, rather than replacing it:

```sh
forma dataset-upload -f ./more-questions.json -a append
```

> 📌 **Note** that a dataset is identified by the `name` field *inside* the JSON file, not by the file's name. So, `more-questions.json` above should say `"name": "wanderer"`.

Datasets written by hand can include reference answers, in an `expected_output` field, and an `id` for each example, which makes them easier to find in the reports:

```json
{
  "name": "wanderer",
  "description": "Questions with reference answers",
  "data": [
    {
      "id": "greeting",
      "input": "hello?",
      "expected_output": "Hi! How can I help?"
    }
  ]
}
```

With this dataset in place, you can now run evaluations and the dataset can be shared with other people.

## Evaluating different parts of an AI Agent

As explained earlier, Forma agents are made up of [4 basic components](../documentation/building-blocks.md):

1. LLMs
2. Nodes
3. Workflows
4. The Agent itself

Sophisticated agents can have several workflows, nodes, and call LLMs multiple times with different instructions and contexts. While Forma allows you to evaluate the agent as a whole (by simply adding an `evals` section in the `agent.yaml` file), it also allows for evaluating each component separately.

### Identifying evaluatable components

In order to see all the components within the agent that *can* be evaluated, you should use the command

```sh
# Use the -f argument if your agent is not in the expected default location
forma eval --list
```

It should print a list with every "evaluatable" element, and the number of evaluations and metrics in each. Here is an example, using the agent from the [Tools tutorial](./tools-introduction.md):

```text
Agent
  - dory - 0 evals | 0 metrics
Workflows:
  - wabe7bfb-b201-4f63-93e6-ebd36c00259c - 0 evals | 0 metrics
  - wccdf7d5-6da5-4fc1-b89c-3ff99983ff75 - 0 evals | 0 metrics
Nodes:
  - translation - 0 evals | 0 metrics
  - blog-post - 0 evals | 0 metrics
  - reply - 0 evals | 0 metrics
  - study-summary - 0 evals | 0 metrics
LLMs:
  - blog-post.llm - 0 evals | 0 metrics
  - translation.llm - 0 evals | 0 metrics
  - reply.summarize-llm - 0 evals | 0 metrics
  - study-summary.llm - 0 evals | 0 metrics
  - reply.llm - 0 evals | 0 metrics
```

> 📌 **Note** that not all elements have evaluations... this will be important later

> 📌 **Also note** that the workflows have a very horrible ID. This happens because, when Forma sees an element without an ID, it assigns one for it. Always assign IDs to any nodes you want to evaluate, as they change every time you load the agent.

### Selecting what to evaluate


Running *ALL* evaluations can be long and tedious. While developing, you are most likley to know what you need to fix and iterate on that. This is why Forma allows you to run evaluations at a per-element basis. For instance:

> 📌 **Note** that the following examples use the `--dry-run` argument. This is meant to help you check what will be evaluated before you evaluate anything. 

```sh
# Evaluate just the agent as a whole
forma eval --dry-run --agent
forma eval --dry-run -a # <-- this also works

## Evaluate Workflow with ID = 'my-workflow'
forma eval --dry-run --workflow my-workflow
forma eval --dry-run -w my-workflow # <-- this also works

## Evaluate node with ID = 'blog-post'
forma eval --dry-run --node blog-post
forma eval --dry-run -n blog-post # <-- this also works

## Evaluate the (triage) LLM of the node with ID = 'reply'
forma eval --dry-run --llm reply.llm
forma eval --dry-run -l reply.llm # <-- this also works

## Evaluate only the evals that use the dataset called 'greetings'
forma eval --dry-run --dataset greetings
forma eval --dry-run -d greetings # <-- this also works

```

> 📌 **Note** that when you define LLMs, they do not have IDs. They are part of a node, and their name will be `<node-id>.llm` (this is the LLM that triages). Nodes with tools also get a `<node-id>.summarize-llm`, which is the one used when the node reaches its `max_iterations` limit (check the [Node](../documentation/building-blocks.md#2-nodes) section).


You can also compound multiple filters by doing something like:

```sh
# Evaluate the agent as a whole AND ALSO workflows called my-workflow and my-other-workflow
forma eval --dry-run -a -w my-workflow -w my-other-workflow

# Evaluate the node 'blog-post', but only its evals that use the 'headlines' dataset
forma eval --dry-run -n blog-post -d headlines
```

> 📌 **Also note** that using the filters to target any element that does not have evaluations **WILL THROW AN ERROR**. The same happens if you pass a `--dataset` name that no evaluation in the agent uses.

### Running fewer examples

While iterating, you rarely need the whole dataset on every run. `--max-examples` (`-m`) caps how many examples each dataset contributes:

```sh
# At most 3 examples per dataset
forma eval -a -m 3 -e quick-check
```

By default the **first** `N` examples are used, in dataset order, so two runs of `-m 3` grade the same examples and stay comparable. Pass `--random` to sub-sample instead; the selection is seeded, so it is still reproducible, and `--seed <number>` changes which subset is drawn:

```sh
forma eval -a -m 3 --random --seed 7 -e quick-check
```

### Running examples in parallel

By default, Forma sends 25 examples to your agent at the same time. If your LLM provider complains about rate limits (or your laptop struggles running Ollama), reduce that number with `--batch-size` (`-b`):

```sh
forma eval -a -b 5 -e slower-but-safer
```

## Evaluating

Now, for evaluating you just need to use the same command as before, but without the `--dry-run` parameter, and providing a value to the `--experiment_name` or `-e` parameter. The experiment name will help organise the evaluation results in a way that you can later compare the effect of your changes against what you previously ran.


```sh
# Note that we removed the -d
forma eval -a -w my-workflow -w my-other-workflow -e my-first-eval
```

After the evaluation is done, the results are in `./evals/results`: one directory per run, each with a `REPORT.md` summarising what every metric made of every answer, and an `experiment.json` per evaluated element holding the same thing in full. The `README.md` next to them lists every element you have evaluated, most recent run first, so that experiments can be compared against one another.

If you ran with `--platform phoenix`, the results are in the Phoenix UI instead.

## Evaluations in CI/CD: required scores

Reports are great for people, but a CI/CD pipeline needs a yes or a no. For this, a metric can declare the minimum score it needs, using the `required_score` field:

```yaml
evals:
  - dataset: wanderer
    metrics:
      - id: politeness
        template: $../evals/metrics/politeness.md
        required_score: 2.5 # <-- HERE
        judge:
          provider: ollama
```

If the mean score of that metric falls below `2.5` on any dataset (or if nothing could be scored at all), `forma eval` says so, marks it with a ❌ in the report, and **exits with an error**:

```text
agent 'dory' · dataset 'wanderer' · politeness: 2.00 is below the required 2.50
Eval Error: 1 metric is below the score it requires
```

This is exactly what you want in CI, and probably not what you want while you are iterating on your agent. In that case, pass `--no-fail`: the breaches are still reported, but the command finishes successfully.

```sh
forma eval -a -e still-iterating --no-fail
```