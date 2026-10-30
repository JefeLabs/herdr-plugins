# Spaces and the run guide

Status: draft for the owner's review. Decisions marked **(owner)** are the owner's; **(my call)** are mine and open to correction.

This is the first of two pieces **(my call, offered to the owner; they went on to design this one)**. The second, user stories and demo videos generated from an intent, builds on it and gets its own design.

## Why

The Intents tool's agent work runs the app in a browser: bug evidence capture, the completion check, and soon demo videos. Today that machinery only works for skoolscout:
- It reads the guide from the intent's own repo (`<repo>/.helmsmith/capture.md`).
- It starts the app with skoolscout's `work start --area …` slots and reads the app's port and folder from `work env`.
- It seeds with `make e2e-story STORY=…`.
- Its log paths are relative to the slot's worktree.

The owner wants the tool to work on any project ("remember this can be used on a project that is not skoolscout"). Projects are often several repos (a backend, a frontend), so how to run the app is a property of the project, not of one repo.

## The model (owner)

- **A space is a project:** one or more repos, matching a Herdr workspace and its `~/intents` group (the owner's model from 2026-10-29).
- **One repo anchors it: the meta repo.** It is either one of the project's repos or an extra repo made for the purpose. Its `.helmsmith/` folder holds the space's shared settings.
- **The meta repo holds the list of the space's repos and how to run the app (owner).** It lives there so teammates share it through git.
- **The other repos sit beside the meta repo as sibling folders (owner).**
- **The tool starts the app itself with commands from the guide (owner).** It does not attach to an app someone already has running.

## What it does

### The space file: `<meta repo>/.helmsmith/space.toml` (shared)

```toml
# The repos of this space, relative to the meta repo, each with where to clone it from.
repos = [
  { path = ".",      git = "git@github.com:jefelabs/tourneyseason-meta.git" },
  { path = "../api", git = "git@github.com:jefelabs/tourneyseason-api.git" },
  { path = "../web", git = "git@github.com:jefelabs/tourneyseason-web.git" },
]
```

- **Paths are relative to the meta repo (owner: siblings).** `.` is the meta repo itself when it is also a project repo.
- **`git` says where to clone a missing sibling from (my call).** When a listed repo is not there, the tool names it and prints the clone command. It never clones anything itself.
- **For skoolscout,** the meta repo is `skoolscout-com`, and its file lists only `.` **(my call)**.

### The run guide: `<meta repo>/.helmsmith/run.md` (shared)

YAML front matter for the tool, then instructions for the agent, as `capture.md` has today.

```markdown
---
start: docker compose up -d db && npm --prefix ../api run dev & npm --prefix ../web run dev
url: http://localhost:{port}         # {port}: a free port the tool picks and passes as PORT
ready: /health                       # polled until the app answers (optional; default: url itself)
seed: ["npm --prefix ../api run seed:demo"]   # run once it answers, in order
stop: docker compose down            # after the run (optional)
logs: [../api/logs/app.log]          # files a check may search, relative to the meta repo
---
How to sign in, test accounts, what to avoid.
```

- **Commands run from the meta repo (my call),** through `sh -c`, with `PORT` set when `url` uses `{port}`. That lets one `start` bring up repos side by side.
- **`start` runs in the background for the whole run.** The tool waits until `url` plus `ready` answers, or gives up after `ready_timeout` (default 10 minutes, **my call**), reporting the last lines of `start`'s output. At the end it runs `stop` if given, then ends `start`'s process group.
- **`seed` commands run after the app answers,** each needing exit 0. A failing one ends the run with its name and exit code, as a failing seed story does today.
- **One run at a time per space (my call),** unless `url` uses `{port}`. Even then, any fixed ports the app itself uses (a database, say) are the guide author's responsibility. The tool says this in an error that names the port in use.
- **Logs stay restricted as today:** relative paths, no symlinks, and canonical containment, here within the meta repo's parent folder (the space's siblings), **my call**.
- **The app runs from whatever the repos have checked out (my call).** The tool does not switch branches or create worktrees for a generic space.
- **Automatic runs wait for the pull (owner: "wait for the pull").** In a generic space, the automatic after-merge check (and a bug's "after" recorded inside it) starts only once the intent's repo's checked-out `HEAD` contains the merge commit of every pull request in that merged set.
  - The tool checks this locally (`git merge-base --is-ancestor <merge commit> HEAD`) and never pulls or fetches.
  - Until then the card says the check is waiting for a pull, and the merged set is not marked as checked, so the check still runs once the code is there.
  - `X` by hand runs on whatever is checked out, as it does now.
  - Skoolscout's `work` path is unaffected, because its fresh slot already merges `develop`.

### Skoolscout keeps its own way (my call)

A guide whose front matter has `default_area` (today's `capture.md` fields) keeps today's behaviour:
- `work start` / `env` / `finish` slots;
- `make e2e-story` seeds;
- the port from `work env`;
- logs relative to the slot's worktree.

So skoolscout's guide (on its unpushed `chore/platform/tooling--capture-guide` branch) works unchanged once it moves to `run.md`. It is read from the space's meta repo, which for skoolscout is `skoolscout-com` itself.

- **File name (my call):** the tool reads `.helmsmith/run.md`, falling back to `.helmsmith/capture.md`, so nothing breaks before a guide is renamed.

### Finding a space's meta repo (each person)

The shared file cannot say where the meta repo is on someone's disk. **(my call)**
- **Each person's Intents config** keeps one line per space: `[spaces.<name>] meta = "<path>"`. The name is the Herdr workspace's, which is also the `~/intents` group the tool already scopes to.
- **Set automatically:** when the tool starts in a workspace with no entry and the current repo (Herdr's workspace folder) has `.helmsmith/space.toml`, it records that repo as the meta repo and says so.
- **Where the guide is looked for, in order (my call, from the builder check):**
  1. the space's meta repo, when one is known;
  2. the intent's own repo, which is today's lookup;
  3. an error, only when neither has a guide: `no run guide: set [spaces.<name>] meta in <config path>, or add .helmsmith/run.md to <repo>`.

  So skoolscout keeps working with no `space.toml` added to skoolscout-com: its guide is found in the intent's repo, as today. Outside Herdr the same order applies, minus the space.

### Who uses it

- **Evidence capture, the completion check and (next) demo recording** read the guide from the space's meta repo, not from the intent's repo.
- **An intent's `repo:`** keeps its meaning: pull requests, specs, plans and the PR search.
- **The capture block** ("no capture guide in …") names the meta repo's path and the expected file.

## Goals

- Captures and checks run on a non-skoolscout project from its meta repo's `run.md`, with no skoolscout tool (`work`, `make e2e-story`) involved.
- A teammate who clones a space's repos side by side and points the tool at the meta repo once gets the same runs as the owner.
- Skoolscout keeps working exactly as today.

## Non-goals

- Cloning repos, switching branches, or creating worktrees for generic spaces.
- Moving intents into the meta repo (still `~/intents`; owner, 2026-10-29).
- Demo videos and user stories. That is the next piece.
- Using the space's repo list elsewhere yet (sending, the PR search across repos). The list is shared and read, but only for the missing-repo message.

## Anti-goals

- Running a guide's commands from anywhere but the meta repo, or with the intent's text in them. Guide commands come from the repo, never from the intent or the agent.
- A run that leaves the app running after it ends (a `start` process group left behind).
- Breaking skoolscout's captures and checks.

## Constraints

- The confined agent is unchanged: browser-only tools, `--restricted --permission-mode dontAsk --tools ""`. The guide's commands run in the tool's runner, never in the agent.
- No push or release without the owner's word. Tests run with stdin from `/dev/null` and never start a real app. They use a fake `start` (a tiny HTTP responder script) and fake seed and stop commands.

## Left to the builder

- `space.toml` parsing details, and how a missing or unlisted repo is reported.
- How `ready` is polled (HTTP via `curl`, or a TCP connect), the poll interval, and how the tail of `start`'s output is kept.
- Where in the config the `[spaces.*]` pointers live (the main config file or a separate one), and the message wording.
- Whether `run.md`'s front matter is checked strictly (unknown keys refused) or loosely.
