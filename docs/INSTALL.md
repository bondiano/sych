# Install

Sych is one binary, `sych`. Locally it reviews the commits between a base branch and `HEAD` and never
publishes anything. As a server it is a GitHub App that comments on pull requests. The fastest way to
set it up is to paste one of the prompts below into your coding agent (Claude Code, Codex, Cursor…)
in the repository you want reviewed. Each prompt holds every step it needs. If you would rather run
the commands yourself, see [Manual install](#manual-install).

## Prompts

### 1. Try it free

Free: no model calls, no keys.

```text
Install Sych 0.2.0 (https://github.com/bondiano/sych) and run a free trial review of this repository.

1. Install the CLI. Download the archive for this machine and its .sha256 file from
   https://github.com/bondiano/sych/releases/tag/v0.2.0 (targets: x86_64-unknown-linux-gnu,
   aarch64-unknown-linux-gnu, aarch64-apple-darwin), verify the checksum, and install the `sych`
   binary into ~/.local/bin (make sure that directory is on PATH). On any other platform run
   `cargo install --git https://github.com/bondiano/sych --tag v0.2.0 --locked sych-app`
   instead (needs Rust 1.94 or newer). `sych --version` must print `sych 0.2.0`.
2. In the repository root, add `.sych-data/` to .gitignore: the local store lives there.
3. Run `sych db migrate`.
4. Run `sych review --repo . --base origin/main --models fake --format markdown`. Use the
   repository's default branch instead of origin/main if it differs. If HEAD has no commits
   beyond it, use `--base HEAD~1`. `--models fake` makes no model calls. The first run downloads
   pinned language servers and linters into ~/.cache/sych/tools.
5. Show me the result and explain it: the verdict, the obligations and why they stay
   UNKNOWN/NOT_ASSESSED without a model, and the coverage gaps. Do not change any source files.
```

### 2. Real model review

This one makes paid calls through [OpenRouter](https://openrouter.ai/), capped by the budgets in
`sych.yaml`.

```text
Set up Sych 0.2.0 for real model reviews through OpenRouter. The `sych` CLI must be installed;
if `sych --version` does not print `sych 0.2.0`, install it first as described in
https://bondiano.github.io/sych/install/.

1. Create the personal config outside the repository:
     mkdir -p ~/.config/sych/config && cd ~/.config/sych
     sych config init --personal
     curl -fsSL -o config/prices.json https://raw.githubusercontent.com/bondiano/sych/v0.2.0/examples/personal/config/prices.json
   `config init` does not write the price catalog, and metered profiles refuse to run without it.
2. The config names its secrets by environment variable. Add these to my shell profile:
     export SYCH_DATABASE_URL=sqlite:.sych-data/sych.db   # relative to sych.yaml
     export SYCH_OPENROUTER_API_KEY=...
   Ask me for the OpenRouter key. Never write it into the repository, a commit or a log.
3. Run `sych db migrate --config ~/.config/sych/sych.yaml`, then
   `sych doctor --config ~/.config/sych/sych.yaml`. The `cheap_api` profile needs only the
   database and OpenRouter variables; the other providers may be reported as unset.
4. Ask me before the first paid call. Then, in the repository:
     sych review --config ~/.config/sych/sych.yaml --models cheap_api --base origin/main --format markdown
5. Summarize the findings, the UNKNOWN obligations and the coverage. Do not edit code yet.
```

### 3. Review before every push

```text
Add a Sych review step to this repository's agent instructions: AGENTS.md, or CLAUDE.md if the
repository uses that; create AGENTS.md if neither exists. Assume `sych` and
~/.config/sych/sych.yaml are set up as in https://bondiano.github.io/sych/install/. Append this
section, with the repository's default branch as the base:

## Review with Sych before every push

1. Run `sych review --config ~/.config/sych/sych.yaml --models cheap_api --base origin/main --format markdown`.
2. Go through every finding and every UNKNOWN obligation:
   - a real problem: fix it and run the review again;
   - a false positive: do not bend the code to the reviewer; explain why in the pull request;
   - `coverage: partial` or listed gaps: report them, never as "no problems".
3. Put the verdict and the open gaps into the pull request description.

Do not run a review now. Add the section and show me the diff.
```

### 4. Self-host the GitHub App

```text
Help me self-host the Sych 0.2.0 GitHub App for my repositories, following
https://bondiano.github.io/sych/server/ step by step. Use the image ghcr.io/bondiano/sych:0.2.0.
It runs `sych serve --config /etc/sych/sych.yaml` as uid 10001.

1. Walk me through creating a GitHub App with the permissions and webhook events the server page
   lists. I click through GitHub myself; you tell me what to set.
2. Write the server sych.yaml from
   https://github.com/bondiano/sych/blob/v0.2.0/docs/examples/server/sych.yaml: my app id,
   repositories and bot login, and absolute storage paths on a mounted volume. Add the
   `openrouter` provider, the `cheap_api` profile and `price_catalog_file` from
   https://github.com/bondiano/sych/blob/v0.2.0/examples/personal/sych.yaml, with the price
   catalog next to it. Start the container with
   `serve --config /etc/sych/sych.yaml --models cheap_api`.
3. Secrets come only from environment variables: SYCH_GITHUB_APP_KEY_FILE (the path of the App's
   private key file), SYCH_GITHUB_WEBHOOK_SECRET, SYCH_API_TOKEN_OWNER, SYCH_DATABASE_URL and
   SYCH_OPENROUTER_API_KEY. Never put their values into files that get committed.
4. Run `sych config validate --config …` and `sych db migrate --config …` with the same image
   before the first start. A non-loopback `http.bind` needs `api_clients` and
   `behind_tls_proxy: true`; put the server behind TLS before GitHub can reach the webhook.
5. Install the App on one test repository, open a small pull request and check that the summary
   comment arrives.
```

## Claude Code plugin

```text
/plugin marketplace add bondiano/sych
/plugin install sych@sych
```

The plugin needs the `sych` CLI on `PATH` and installs nothing itself.

- `/sych:review [--base REF] [--config PATH --models PROFILE]` reviews the committed changes of the branch against
  the remote default branch. It uses `--models fake` unless you pass a profile, and asks before the first paid
  call. It then reports the recommendation, coverage, findings, UNKNOWN obligations and unreviewed paths.
- Fixes go through `sych handoff` with your consent.
- The `sych-review` skill applies the same steps before a push.

## Manual install

Prebuilt binaries cover Linux x86_64 and aarch64 (glibc 2.35+) and macOS on Apple silicon:

```sh
version=0.2.0
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  *) echo "no prebuilt binary for this platform: build from source" >&2 ;;
esac
base="https://github.com/bondiano/sych/releases/download/v$version"
curl -fsSL -O "$base/sych-$version-$target.tar.gz" -O "$base/sych-$version-$target.tar.gz.sha256"
shasum -a 256 -c "sych-$version-$target.tar.gz.sha256"
tar -xzf "sych-$version-$target.tar.gz"
mkdir -p ~/.local/bin && install -m 0755 "sych-$version-$target/sych" ~/.local/bin/sych
sych --version
```

From source, with Rust 1.94 or newer:

```sh
cargo install --git https://github.com/bondiano/sych --tag v0.2.0 --locked sych-app
```

The server image is `ghcr.io/bondiano/sych:0.2.0`.

At run time Sych needs `git` (2.40 or newer for `.gitattributes` generated-file marking). Language
servers and linters are pinned downloads into `~/.cache/sych/tools`, never taken from `PATH`; Rust
semantic facts also need the host `cargo`. `--semantic off` skips all of them.

Then:

```sh
sych db migrate
sych review --repo . --base origin/main --models fake --format markdown
```

[Local review](LOCAL_REVIEW.md) explains the output, formats and exit codes. [Configuration](CONFIG.md)
covers `sych.yaml` and the repository policy in `.sych/`, and [Models](MODELS.md) covers providers,
profiles and budgets.
