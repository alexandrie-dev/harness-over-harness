# HoH first version: what the founder asked for (18 November 2026)

Source: the founder's own answers in `plans/vision-1118.md` (Russian). This file restates them for executors;
where the two differ, the founder's file wins.

## The product

HoH (working name of the engine: Alexandrie Harness) is a desktop harness for ordinary people, not for
programmers. The founder likes T3 Code (`/mnt/toshiba/alexandrie/hoh/refs/t3code`, MIT) and wants "the same, but
more customizable"; T3 Code is about code, HoH is a personal assistant. Above all the product must be easy to
learn.

A typical user: a man who sells IT equipment. His tasks: watch tenders, read documents, keep an eye on the price
of a GPU so that he does not sell too cheap. A personal assistant.

## First version

- One engine only: our own, made after DeepSeek Harness (dsh, MIT,
  `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`): its logic and scaffolding plus our improvements for
  creating a virtual assistant. Other harnesses inside HoH (Claude Code, Codex) are not needed in the first
  version. The engine comes first, the window after it.
- From T3 Code the founder wants: preview of results inside the window (pages, pictures, charts); schedules and
  timed tasks; the look and the speed of the interface.
- A convenient interface for creating **virtual employees** of a company.
- Customization: the user's own buttons and commands; own agent roles and rules for them; own tools that the
  model writes on request ("a task is a plugin": the user describes a task, the agent asks questions, shows an
  example, and after "yes" the task runs on rails).
- The interface may be written in TypeScript if that is cheaper and looks better (the earlier decision was
  Tauri 2 with a Rust/Leptos interface). Condition: a scheme that protects us from supply-chain attacks.
  Security is to be shown as a large feature of the product.
- Unchanged earlier decisions: local Qwen as the main model for ordinary users; marketplace after the first
  version; Linux first, then Windows and macOS; closed source, signed binary, auto-update.

## The founder's acceptance example

"I want a tool that analyses a person by his Telegram account: how to talk to him, whether he needs an exact
plan or a vision. The agent goes to the internet, collects books on psychoanalysis, digs through them, then
learns to parse a Telegram account (photos, bio, texts), and afterwards I can come in and run it as a skill. I
could also create a virtual assistant, a psychoanalyst. All of it by hand from the UI."

So the first version must let a user, from the UI and without a programmer: describe a new task; have the agent
research sources on the web and build tools for it; get a reusable skill; create a virtual employee with a role
that uses that skill; run it on demand or on a schedule; see the result previewed in the window.
