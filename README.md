# Skep

Skep runs the services a project needs on macOS natively: Postgres, MySQL,
MongoDB, Valkey and Mailpit, with no Docker and no Homebrew. It has a command
line, a window, and an MCP server, so an agent can manage services in one call
instead of working it out through `brew`, `ps` and `lsof`.

Apache-2.0 OR MIT. Nothing is paid, and there is no account.

## Two things it can prove

### Branches are real databases, running at the same time

A branch is a second Postgres on a copy of the first one's data, on its own
port. Both are live; neither can see the other's writes. This is a transcript
of the commands, not a description of them.

```
$ skep branch postgres experiment
stopping postgres@17.6.0 to copy its data
postgres@17.6.0:experiment 55146 (branch)

$ psql -p 15432 -c 'select body from notes'   # main
on main
$ psql -p 55146 -c 'select body from notes'   # the branch, same data
on main

$ psql -p 55146 -c "update notes set body = 'only in the branch'"
$ psql -p 55146 -c 'select body from notes'   # the branch
only in the branch
$ psql -p 15432 -c 'select body from notes'   # main, untouched
on main
```

On APFS the copy is `clonefile`, so it is copy on write and effectively
instant. Where a clone cannot apply, skep says "copy the data, which is not
instant here" before it starts rather than after.

The same behaviour is asserted in
[`crates/comb-services/tests/branches.rs`](crates/comb-services/tests/branches.rs),
which runs two servers concurrently and checks each reads back only its own
writes.

### One tool call instead of four shell commands

Asking "what dev services are running, and is the database ready?" costs an
agent 4,431 tokens of shell output. Asking skep costs 387.

```
tokens, counted by claude-sonnet-5

  shell, 4 commands     4431   bench/manual.txt
  skep, one call          387   bench/skep.txt

  11.4x cheaper
```

Both transcripts are real command output rather than an invented baseline, and
they are in [`bench/`](bench/) so the number can be checked rather than
believed. Identifying details are replaced before the file is written: the
username, the machine's name, and any address outside loopback. Each file says
at the top exactly what was substituted and how many times. The process list
itself is untouched, because that is the thing an agent would have to read.
[`scripts/token-benchmark.sh`](scripts/token-benchmark.sh) regenerates both.

Three caveats, because the number is only worth what its method is worth:

1. Counted by Anthropic's `count_tokens` endpoint against claude-sonnet-5, on a
   MacBook Pro (M1 Pro, macOS 27). Run the script without `ANTHROPIC_API_KEY`
   and it prints a rough character estimate instead, clearly labelled: that
   estimate understates shell output badly, since process lists and port tables
   tokenise far worse than four characters per token.
2. The two sides do not survey the same scope: `skep_status` answers for the
   services skep manages, the shell commands survey the whole machine. An
   agent cannot narrow the shell search without already knowing the answer,
   which is the point, but they are not identical questions.
3. The size of the shell side depends on how much the machine has installed.

## Sixty seconds

Requires macOS on Apple silicon and Rust 1.96 (the toolchain is pinned, so
`rustup` fetches it).

```sh
git clone https://github.com/quirelabs/skep
cd skep
cargo build --workspace
```

Describe what a project needs, in `skep.toml` at its root:

```toml
[services.postgres]
version = "17"

[services.mailpit]
```

Put the command somewhere your shell can find it, then host the engine in one
terminal and bring the project up in another:

```sh
export PATH="$PWD/target/debug:$PATH"

skep serve                         # holds the services; ctrl-c stops them
cd path/to/your/project && skep up
```

```
/Users/you/code/your-project/skep.toml
  mailpit@1.31.0 started
  postgres@17.6.0 started
```

Skep downloads a pinned, checksummed build of each service on first use and
runs it directly. Nothing is installed system wide, and everything lives under
`~/.skep`, which you can delete.

If something already holds a port, skep says what and offers the fix rather
than failing with an exit code:

```
  postgres: port 5432 is held by postgres (pid 2517, Homebrew). Stop it with
  `brew services stop postgresql@17`, or change the port in skep.toml.
```

For the window, which hosts the engine itself and stops services when it quits:

```sh
cargo run -p skep-app
```

Other commands: `skep status`, `skep start|stop|restart <service>`,
`skep logs <service>`, `skep snapshot <service> <name>`,
`skep branch <service> <label>`, `skep branches`. Run `skep help` for the rest.

## For agents

Wire the MCP server into a client by pointing it at the built binary:

```json
{
  "mcpServers": {
    "skep": {
      "command": "/absolute/path/to/skep/target/debug/skep-mcp"
    }
  }
}
```

The server is a client of the engine, not a second copy of it. If no engine is
running it says so and stays up:

```
no skep engine is running. Start one with `skep serve`.
```

| Tool | What it does |
| --- | --- |
| `skep_status` | Every service, its state, ports, current phase, and why anything failed |
| `skep_start` | Start a service, installing it first if needed |
| `skep_stop` | Stop a service |
| `skep_restart` | Restart a service |
| `skep_logs` | A bounded tail of a service's output |
| `skep_project` | Read a repository's `skep.toml` and report or start what it needs |
| `skep_snapshot` | Keep a named copy of a service's data |
| `skep_snapshots` | List the copies kept |
| `skep_branch` | Run a second copy on its own data and port |
| `skep_delete_branch` | Remove a branch and its data |

Every tool answers with the resulting state, so nothing needs a follow up call.
Errors are the same sentences a person gets, including the remedy, because
agents relay them verbatim. One `skep_status` call, verbatim:

```json
{"services":[
  {"id":"mailpit@1.31.0","state":"ready","ports":{"http":8025,"smtp":1025},"pid":36939},
  {"id":"mongodb@8.0.29","state":"stopped","ports":{"mongodb":27017}},
  {"id":"mysql@8.4.6","state":"stopped","ports":{"mysql":3306},
   "blocked":"port 3306 is held by mysqld (pid 2765, Homebrew). Stop it with `brew services stop mysql@8.4`, or change the port in skep.toml."},
  {"id":"postgres@17.6.0","state":"ready","ports":{"postgres":15432},"pid":36970},
  {"id":"valkey@9.1.1","state":"stopped","ports":{"valkey":6379},
   "blocked":"port 6379 is held by redis-server (pid 2505, Homebrew). Stop it with `brew services stop redis`, or change the port in skep.toml."}
]}
```

A branch is a sibling, not a child: it belongs to a service and version, so
branching a branch gives another sibling rather than a nested one.

## What is in here

| Path | |
| --- | --- |
| `crates/comb` | The engine: supervision, readiness, orchestration, snapshots. Publishes as `quire-comb` |
| `crates/comb-services` | The service catalog and its pinned releases. Publishes as `quire-comb-services` |
| `crates/skep-cli` | The `skep` command |
| `crates/skep-mcp` | The MCP server |
| `crates/fake-service` | A test fixture: a process the supervision tests can steer. Not a product |
| `app/skep` | The macOS app, built on GPUI |
| `scripts/pin-release.sh` | Records a service release and its hash for the catalog |
| `scripts/token-benchmark.sh` | Produces the transcripts in `bench/` |

The engine is a library with no opinion about interfaces. The command line, the
MCP server and the app are all clients of it, which is why they cannot disagree
about what is running.

crates.io carries the two libraries only. The command line, MCP server and app
are products rather than dependencies and ship as release binaries.

## Where it runs

macOS on Apple silicon. Every pinned release in the catalog is an arm64 build,
and CI refuses to run on anything else rather than testing the wrong
architecture.

Linux is the intended second platform: all OS specific code is confined to one
module, one file per system, and Valkey already publishes Linux binaries that
would replace the source build macOS needs. Windows is out of scope
indefinitely.

## Tests

```sh
cargo test --workspace                        # skips the heavy adapters, loudly
SKEP_TEST_HEAVY=1 cargo test --workspace      # boots MySQL, MongoDB and Valkey too
```

Both report the same number of tests: the heavy ones return early rather than
disappearing, so the printed `SKIPPED` lines are the only signal for which mode
ran. CI runs both, plus a test that re-downloads a pinned release and checks it
still matches its recorded hash.

Every service adapter is tested by starting the real binary and speaking its
real protocol. Readiness is never a sleep.
