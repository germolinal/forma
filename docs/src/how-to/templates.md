# Templates

Templates work by replacing a *Template Field* with the value of a variable with a certain *Field name*. And it is very simple:

If you have a variable `some-variable=2`, and a template saying `I have {{some-variable}} apples`, rendering the template will give you `I have 2 apples`. That is all, simple and predictable.

There are two main situations where templates become very useful:

1. Using dynamically generated content (e.g., the response of one agent) as part of the input to another Agent or LLM.
2. Defining reusable blocks of instructions to avoid writing the same thing multiple times

Forma only allows for the first use case, and **does not support reusable blocks of instructions** (more on this [later](#-why-not-reuse-blocks-of-instructions)).

## Templates for dynamically generated content

Templates can include fields for dynamically generated content. For example, you can have a node called `descriptor` that produces a short product description; and then a node `pr-expert` in charge of adapting it so that it matches the tone and style of your company. 

Imagine your `descriptor` is given the following isntructions.

```md
Users will send you a picture. Your job is to describe the 
product in such picture.
```

And then your `pr-expert` will be given the following instructions:

```md
You are a PR expert, in charge of improving product descriptions
so that they match the following guidelines:

1. No emojis
2. Always describe in a positive tone

We need you to improve the following description:

{{descriptor}}
```

### Naming nodes and fields

Template fields are identified by variable names. While not very limiting, these names need to follow a few simple rules:

* Must start with a letter (a–z or A–Z) or an underscore (_).
* Can contain letters, digits, underscores (_), and dashes (-).
* Cannot start with a digit.
* Cannot contain spaces, dots, or special symbols (like $, %, /, .).

Here are some examples:

| Variable Name | Valid | Reason                         |
| ------------- | ----- | ------------------------------ |
| `world`       | ✅     | Simple lowercase name          |
| `_user`       | ✅     | Starts with underscore         |
| `big-world`   | ✅     | Dashes are allowed             |
| `user_123`    | ✅     | Numbers allowed (not at start) |
| `HelloWorld`  | ✅     | Uppercase letters allowed      |
| `1world`      | ❌     | Cannot start with a digit      |
| `user name`   | ❌     | Spaces not allowed             |
| `user.name`   | ❌     | Dots not allowed               |
| `user/name`   | ❌     | Slashes not allowed            |
| `$user`       | ❌     | `$` not allowed                |


### Describing fields

Sometimes it is very useful to communicate to people or LLMs what the different fields are supposed to be. For instance, when executing a Workflow as a tool, this Workflow is given an `input_prompt`. This is a template that will be rendered and used as the first message received by said workflow, and an LLM will need to dynamically generate all the variables needed to fully render this template. Giving this LLM some information about what the fields are makes them behave much more reliably.

Fields can be fully described within a template using the following syntax:

```md
{{name:optional-type "optional-description"}}
```

Where:

* `name` is the mandatory field name, as described above.
* `type` tells us what kind of value is expected (e.g., an int? float? string?). If not given, it is assumed to be a string.
* `description` is just some free text that will tell the LLM or the user what this is meant to be

The following are the valid types:

| Type                | Description       | Examples                        |
| ------------------- | ----------------- | ------------------------------- |
| `string`            | Any text text     | `"car"`, `"bananas with syrup"` |
| `number`            | Any number        | `1`, `31`, `99.2123`            |
| `int` or `integer`  | An integer number | `2`, `212`                      |
| `bool` or `boolean` | True or false     | `true`, `false`                 |

The following are examples of valid and invalid fields

| Template Expression                               | Valid | Reason                                              | Type      |
| ------------------------------------------------- | ----- | --------------------------------------------------- | --------- |
| `{{ world }}`                                     | ✅     | Basic variable, with no type. Defaults to `string`. | `string`  |
| `{{ name:string }}`                               | ✅     | Includes a type                                     | `string`  |
| `{{ age : int }}`                                 | ✅     | includes valid type                                 | `int`     |
| `{{ name:string "Full name" }}`                   | ✅     | Includes optional description                       | `string`  |
| `{{ is_big:bool "Is larger than 3 elephants?" }}` | ✅     | Underscore prefix and description                   | `boolean` |
| `{{ 1world }}`                                    | ❌     | Invalid name                                        | --        |
| `{{ user:"string" }}`                             | ❌     | Type should not be quoted                           | --        |
| `{{ world: string "Mismatched quotes }}`          | ❌     | Missing closing quote in description                | --        |
| `{{ age : ints }}`                                | ❌     | Invalid type                                        | --        |




## ❌ Why not reuse blocks of instructions

Some frameworks have a very powerful prompt templating system, which allows users to reuse pieces of prompts in several places. Forma does not really do that, on purpose. The reason is that this makes it harder to [audit AI Agents](../intro.md#3-agents-should-be-auditable). Take the following code as an example:

```py
import llm
from prompts import tone_and_style, safety

prompt = f"""
You are a customer service agent. Your job is to help customers
achieve their goal.

{tone_and_style}
{safety}
"""

llm.chat(prompt, "hello!")

```

This syntax is appealing. It looks neat and modular. And yet, ask yourself:

* What will be the exact prompt that the LLM will receive? 
* Do you know if the `tone_and_style` and the `safety` blocks have a header? 
* Are they both written in Markdown?
* Do they have blank lines at the end?
* Do they start with a "You are a..." section?

You can go and check these files, of course, but that introduces friction and the risk of having incoherent overall instructions. 

In summary, Forma avoids this because:

* **Prompts are not modular code** — they are natural language instructions that need to make sense as a whole.
* The **final rendered prompt must always be auditable**, reproducible, and human-readable.
* Reusable “blocks” create hidden dependencies that **make debugging, auditing, and comparing behavior much harder**.
