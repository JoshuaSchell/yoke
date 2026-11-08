# Yoke

A harness for harnesses: one desktop app for driving coding agents (Claude Code, Codex,
OpenCode) across all your projects, after T3 Code. It's built on
[Rune](https://github.com/anomalyco/rune), a GPU-rendered Rust UI framework. It's young, not a
full T3 Code yet. It was called Rune Code until the rename; its first launch as Yoke copies the
old settings and saved threads over.

![dark](shots/demo-dark.png)

Usage: the Claude plan's limits and what Claude Code has spent (⇧⌘U):

![Usage](shots/usage-dark.png)

Nord dark and Gruvbox light:

![Nord dark](shots/nord-dark.png)
![Gruvbox light](shots/gruvbox-light.png)

## Run

```sh
cargo run --release                 # opens on the project used last
cargo run --release -- ~/code/app   # or adds and opens a project folder
```

The sidebar lists every project's threads together, as T3 Code does, each row naming its project. Each row starts with its project's mark: the project's own icon when the folder has one (a favicon, or an app icon such as `assets/icon.png`), else its initials in a colour no other project has. ✎ on a project in the headline's picker sets it: automatic, an image, or letters and a colour. The new-thread screen's headline picks the project ("What should we build in **yoke ▾**?"): click the name to search your projects, add one (or **⌘O**), or take one out with the × under the pointer (its folder and saved sessions stay). On the first launch, the folders Claude Code has sessions in become the projects, temporary folders aside. Where the app is started from doesn't matter: it opens the project used last.

## Diffs

Each turn's changes, after T3 Code's: just before a turn's CLI starts, Yoke snapshots the folder in git, and again when the turn ends, so the diff between them is what the turn changed, edits made through shell commands included, whichever agent ran it. A turn that changed files ends with a chip (**2 files +20 −1**) that opens the diff panel on it; **⇧⌘D** or the toolbar's button opens it on the last turn. Its menu also offers **Uncommitted changes** (HEAD to the working tree, new files too) and **Branch changes** (from where the branch left main). The panel shows unified or split, folds unchanged lines (click to open them), highlights the words that changed, and uses the theme's code colours.

A snapshot is a commit made with a scratch index (`git add -A`, `write-tree`, `commit-tree` on HEAD), kept under `refs/yoke/<token>/{start,end}`: your branches, index and stash aren't touched, and ignored files aren't included. It takes about 20–60 ms in a repository of a few thousand files. Deleting a thread deletes its refs; a small index per session (`checkpoints/` beside the saved threads) remembers which turn had which. A folder outside git has no turn diffs.

## Install

Download the `.dmg` from [Releases](https://github.com/JoshuaSchell/yoke/releases) (Apple silicon), open it and drag Yoke to Applications. It isn't notarized yet, so the first time, right-click it and choose Open.

## Updates

The installed app keeps itself up to date. At launch and every 6 hours it reads `latest.json` from the newest GitHub Release, and when there's a newer version it downloads the update bundle in the background, checks its signature against the public key built into the app (`assets/updater.pub`), and replaces the app on disk. The toolbar's circling arrow (by the usage gauge) checks on a click and turns while it checks and downloads; its tooltip says where things stand. Once an update is installed the same button becomes **Restart to update** (held back while a turn runs). Settings › Advanced shows where it stands too, and has **Check now**. A build run from source (`cargo run`) never updates itself. The code is `src/updater.rs`, on cargo-packager's updater.

The release workflow signs the bundle with the private half of that key, from the repo secrets `CARGO_PACKAGER_SIGN_PRIVATE_KEY` and `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`. Keep a backup of the key: an app that can't verify an update won't install it, so losing the key means reinstalling once by hand. To try an update locally against a release's files: `YOKE_UPDATE_FILES=<folder with latest.json and the .app.tar.gz> cargo test -- --ignored an_update_installs --nocapture`.

## Release

Bump `version` in `Cargo.toml`, commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`. The Release workflow (`.github/workflows/release.yml`) tests, packages and publishes a GitHub Release with the `.dmg`. Run it by hand from the Actions tab for a dry run that only builds the `.dmg`. The CI workflow runs the tests on every push to `main`.

## Package

```sh
cargo install cargo-packager --locked   # once
cargo packager --release                # target/release/Yoke.app and Yoke_<version>_<arch>.dmg
```

On Windows the same command builds an NSIS installer and on Linux a `.deb` and an `.AppImage` (untested so far). The config is `[package.metadata.packager]` in `Cargo.toml`. The macOS bundle is ad-hoc signed, with no Apple account: copied from the `.dmg` after a browser download, it needs right-click › Open the first time. The icon, a white glass ox's head on black, is an Icon Composer document, `assets/Yoke.icon`; `scripts/icons.sh` renders the `.icns`, `.png` and `.ico` from it with Icon Composer's `ictool`.

- **Return** sends and **Shift-Return** adds a newline (or, with Settings › Chat › Send with set to ⌘ Return, **⌘Return** sends and Return adds a newline). **Esc** stops a running turn, killing the CLI's whole process group so a tool command it started can't keep the turn alive.
- **While a turn runs**, Return **queues** the message: queued messages sit above the prompt and go in order when the turn ends. **⌘Return** (or the ↗ button on a queued message) **steers**: it stops the turn and sends the message straight away on the same session. After Esc the queue waits until you send from it. Settings › Chat › Follow-ups can make sending steer instead; the Steer key (⌘Return, or ⇧⌘Return when ⌘Return sends) then queues.
- **⌘N** starts a new thread, **⌘O** adds a project, **⇧⌘O** opens the thread's folder in your editor (below), **⇧⌘D** shows or hides the diff, **⇧⌘U** shows usage and plan limits, **⇧⌘B** shows or hides the sidebar, **⇧⌘P** shows frame stats, and **⌘,** opens Settings. Settings takes over the main pane, with its pages down the left as in T3 Code; Esc, Done or picking a thread closes it. The pages:
  - **Appearance**: theme, and light, dark or the system's.
  - **New threads**: what a new thread starts with: the agent (CLI), the access level, where it runs in a git repository (Local, or a New worktree), and each agent's model and reasoning effort. ⌘N and the first screen at launch start from these; switching agent on the new-thread screen takes that agent's; the composer can still change them before you send. Reset goes back to the CLIs' own defaults.
  - **Chat**: Send with (Return, or ⌘ Return for long prompts) and Follow-ups (queue or steer while a turn runs).
  - **Agents**: which CLIs were found, and how many models each reported. Look again searches once more (after installing one, say). **Messages from your other sessions** (on by default) lets your other Claude Code sessions, here or in a terminal, message a Claude Code thread here (`SendMessage`), for example to say which files they're editing. Each turn passes `--settings '{"crossSessionInbound":"accept"}'`, or `"refuse"` with the switch off. Without either, the CLI holds a message from a session in a different permission mode for an approval a thread here can't give, so the message would expire unread. A message can't give a thread more access than it has.
  - **Skills**: every skill the agents can read, yours and the current project's, with which agents see each. Codex and OpenCode read `~/.agents/skills`, Claude Code reads `~/.claude/skills`, and OpenCode reads both, so a skill added for one agent isn't seen by the others. **Share** links the skill's folder into the folders of the agents that don't see it (in a project, a relative link you can commit); nothing is moved or copied, so a skill another tool installed stays where it put it. **Stop sharing** removes only the links Yoke made. **New skill…** makes one in `~/.agents/skills` with a SKILL.md to fill in, linked for Claude Code. Skills synced from claude.ai stay Claude Code's; two separate copies with one name are flagged.
  - **Keyboard shortcuts** (below).
  - **Advanced**: frame stats, the settings file (Show in Finder), and the version.
- **Every shortcut can be changed** in Settings › Keyboard shortcuts. Click one and press the new keys: Esc cancels, ⌫ clears, and a chord that's already in use is refused. The list covers new thread, sidebar, settings, frame stats, focus prompt, previous/next thread (⌥⌘↑/↓), settle thread (⇧⌘S), delete thread (⇧⌘⌫), stop, steer, and opening the pickers (⇧⌘M for the model picker, which ⇧⌘H, ⇧⌘E and ⇧⌘A also open). Changes save to `~/Library/Application Support/dev.yoke/prefs.json`, along with the appearance. The menu bar's Settings… item picks up a new chord at the next launch.
- The window answers macOS tiling (Window › Fill, Center, Move & Resize, and their Fn-Control shortcuts).
- The composer has a paperclip and one picker, as T3 Code's does:
  - **the paperclip** attaches a file (see Attachments below).
  - **the model picker** (⇧⌘M, after T3 Code's): one chip ("Opus 5.5 High") opening a popover with Favorites and each harness down a rail, a search over the harness's models, the reasoning effort beside them, and the access level along its foot.
    - The **harness** is fixed once a thread starts, since sessions don't move between CLIs; a harness that isn't installed is dimmed.
    - **Models** are asked of the CLI at launch (see below). The first five show until you search or press Show more. ⌘1–⌘9 pick a row by number, ↑↓ and Return pick too, and Esc closes.
    - The **star** on a row pins the model to Favorites, which are saved in prefs.
    - **Effort** is Claude Code `--effort` (low … max) and Codex `model_reasoning_effort`. Its column is hidden for harnesses with no effort flag. Default sends no flag, so the CLI picks.
  - **Access** (along the popover's foot, Full access marked with a warning sign): Read only, Ask first, Edit files or Full access. With Claude Code, anything beyond the level you pick asks you in the app (see Approvals) instead of being refused:
    - **Ask first** lets reads through and asks before edits and commands;
    - **Edit files** lets edits through and asks before commands;
    - **Full access** never asks;
    - **Read only** refuses without asking.

    Changing the access picker while a Claude Code turn runs applies to that turn straight away (the Agent SDK's `set_permission_mode` control request; turns start with `--allow-dangerously-skip-permissions` so Full access can be switched on mid-turn), and switching to Full access allows the approvals waiting. Switching to Edit files doesn't: an edit Claude Code still asks about under Edit files (outside the project, or a settings file) is one it wants you to look at. Read only can't be switched on mid-turn. A queued message runs at the access level shown when it starts, not when it was queued.

    The other CLIs can't ask in their one-shot modes, so for them this choice is made up front. Ask first isn't offered for them. OpenCode's `run` takes no permission flags Yoke could set, so for it the popover says its own config decides, rather than offering levels that wouldn't apply.

## Usage and plan limits

Two speedometers, whose needles swing as the numbers change. Hover either for a card with the figures.

- **The composer's** shows how full the thread's **context window** is (Claude Code threads; the other CLIs don't say). It reads green, amber from 50% and red from 90%. A new thread's is at zero. Its card shows a bar, the tokens used and left, and a note when the window's size is still a guess or it's nearly full. The figure is the latest model call's input, cache reads and writes, and output, from the CLI's stream (a subagent's calls aside, since they have a context of their own), and for a reopened thread, the last reply in its transcript. The window's size is what Claude Code reported for the model after a turn; it's remembered per model, and until then it's taken as 1M for a `[1m]` model and 200K otherwise.
- **The toolbar's** shows the **plan's** tightest window. A plan is there to be used, so it reads the other way: red below 50%, amber from 50% and green from 90%. Its card has every window's meter (how much is used, the even-pace tick, when it resets and the pace), as on the Usage page. Clicking it (or **⇧⌘U**) opens the Usage page, with how close the Claude plan is to its limits and what Claude Code has spent, as T3 Code's usage page does.

- **Plan limits.** Each window the plan has (the 5-hour session, the week, a model's own weekly allowance such as Fable's) gets a meter: how much is used, when it resets, and a tick where even spending would be by now. "Ahead of pace" means you're spending faster than the window passes. Past 75% a meter turns amber ("Near limit"), and past 90% red.
  - The numbers come from the CLI's `get_usage` control request (what the Agent SDK's usage call and `/usage` read), sent to `claude -p --input-format stream-json` with no prompt, so no tokens are spent. This and the model lists are asked from an empty folder of Yoke's own (`probe` in `dev.yoke`), not a project, so a repository's own settings (Claude Code hooks, OpenCode's MCP servers) don't run when nobody asked for a turn. It answers in about half a second.
  - They're read at launch, after every Claude Code turn, and every 5 minutes. During a turn the CLI's `rate_limit_event`s move the session and weekly meters live. The last reading is saved, so the gauge has numbers at the next launch straight away.
  - Signed in with an API key (no plan), there are no windows, and the page says so.
- **Spend at API prices.** Today, 7, 30 or 90 days: cost, tokens, the share of input read from cache, a cost-by-day chart (hover a day for its figures), and tables by model and by project.
  - It's built from Claude Code's own transcripts (`~/.claude/projects/**/*.jsonl`, or each `$CLAUDE_CONFIG_DIR`), so terminal sessions count as well as the app's. A message is counted once by `message.id` + `requestId`, because the CLI writes one line per content block and a resumed session repeats its history. Only transcripts that changed are read again: 25 transcripts took 21 ms here, and a rescan 0.6 ms.
  - Prices are LiteLLM's table (T3 Code's source too), fetched once a day with `curl`. Only its Claude rows are kept in prefs. Offline, a built-in copy (`src/usage_prices.json`) prices them. 1-hour cache writes, fast mode, US-only inference and the long-context tiers each get their own rate. Checked against the CLI: a Haiku turn it billed at $0.00207222 prices to exactly that.
  - On a subscription this is what the same tokens would cost on the API, a measure of use rather than a bill.

## Uninstall

Yoke keeps `prefs.json` (shortcuts, appearance, theme, new-thread defaults, the send and follow-up settings, the projects and the last one used, settled threads, the model lists, favorite models, the last plan-limits reading and Claude's prices) and a `threads` folder (Codex and OpenCode threads, one JSON file each) in `dev.yoke` in the platform's config folder. Pasted screenshots wait in the temp folder's `yoke-pastes` and are deleted a day later, at the next launch. On macOS, the system also keeps a GPU shader cache for the app.

| Installed with | Remove the app | Remove its data |
| --- | --- | --- |
| `.dmg` (macOS) | Drag Yoke from Applications to the Trash | `rm -rf ~/Library/Application\ Support/dev.yoke "$(getconf DARWIN_USER_CACHE_DIR)dev.yoke"` |
| Homebrew cask (macOS) | `brew uninstall --zap yoke` | The cask's `zap` removes the same two folders |
| Nix | Drop it from your config (or `nix profile remove`) | As for the `.dmg` on macOS; `rm -rf ~/.config/dev.yoke` on Linux |
| `.deb` / `.AppImage` (Linux) | `sudo apt remove yoke`, or delete the `.AppImage` | `rm -rf ~/.config/dev.yoke` |
| Installer (Windows) | Settings › Apps › Yoke › Uninstall | Tick "Delete the application data" in the uninstaller |

Uninstalling leaves the agents' own files alone, since the terminal CLIs share them: Claude Code's session transcripts (`~/.claude/projects/`), and "Always allow" rules saved to a project's `.claude/settings.local.json`.

## Trusting a project

Before a project's first turn, Yoke asks **Trust this project?** The CLIs' print modes skip their own trust prompt, and a project can carry settings that run commands as soon as a turn starts (Claude Code hooks, MCP servers), so a folder you cloned but haven't looked at shouldn't get to run them unasked. **Trust and send** (Return) remembers the folder in prefs and sends the message; **Cancel** (Esc) keeps the draft and sends nothing. A folder inside a trusted one is trusted too, and so is any folder Claude Code itself was told to trust (`hasTrustDialogAccepted` in `~/.claude.json`), so projects you already use from the terminal don't ask.

## Approvals

When Claude Code wants to run a tool your access level doesn't allow, the tool's row shows everything it would do: the command, path or URL, then every other field of its input as JSON (a file's content, an MCP tool's arguments), scrolling rather than cut short, with characters that hide or reorder text (bidirectional controls, zero-width marks) shown by their code. Then **Deny**, **Always allow** and **Allow**. The turn waits until you answer, and the sidebar marks the thread "Needs approval".

- **⌘Y** allows and **⇧⌘Y** always allows. Esc stops the whole turn.
- **Always allow** saves the rule Claude Code suggests (for example `Bash(cargo check:*)`) to the project's `.claude/settings.local.json`, and the card names the rule before you click. Only rules that allow, kept in the project's local settings or the session, are saved: a suggestion to change the permission mode, add a folder, or write your global settings is dropped, and with none left the button isn't shown. That rule then applies to every later session, including ones run from the terminal.

This works the way the Claude Agent SDK talks to the CLI. The prompt goes in on stdin (`--input-format stream-json`), and `--permission-prompt-tool stdio` makes the CLI send a `can_use_tool` control request and wait for the app's `control_response`. stdin is closed after the turn's result so the CLI exits, so each turn is still one process.

## What's there

- The sidebar is an inbox, after T3 Code's:
  - At the top, with no heading: every project's threads that need you in one list, the last to come back first, each naming its project when there's more than one. A label in each card's corner says why: **Approval** (a tool waits for you), **Failed** or **Done** (a turn ended since you last looked). A card with nothing for you shows how long ago it was active instead, and fades back.
  - **Working**, a shelf at the sidebar's foot, shut by default: threads with a turn running and nothing asked of you, each with how long it has run and a spinner. Shut, it still shows the thread you're reading.
  - **Settled**, below it: threads you're done with. **⇧⌘S** settles the open thread and goes on to the next one in the inbox, so you can work down to "All caught up". Hovering a row shows ✓ (settle) or ↩ (back to the inbox) and × (delete). A settled thread comes back to the inbox as soon as it does anything: a message sent, an approval asked, or a turn finished. Threads untouched for 7 days settle on their own (T3 Code's default; Settings › Chat › Settle after takes 1 to 30 days, or Never): checked at most once a minute while the app is in use, and otherwise only when the next one is due, so an idle window doesn't wake for it. A worktree thread also settles once its branch is merged, as T3 Code settles on merge: when main (local or on `origin`) holds the branch's commits, or its pull request is merged or closed (asked of `gh`, if it's installed and signed in). That's looked at every few minutes, and a thread that does something after its merge isn't settled by it again. Running, asking or queued threads, and ones you moved back to the inbox by hand, never auto-settle. The shelf is shut by default, scrolls on its own once it's 320 points tall, lists 30 at a time, and still shows the thread you're reading when shut. What you settle is saved by session id in prefs, so it holds across launches. **× (or ⇧⌘⌫) deletes** after asking (Return deletes, Esc keeps it): the thread goes, and for a Claude Code thread so do its session's files, so it doesn't come back next launch. That means the transcript (`~/.claude/projects/<folder>/<id>.jsonl`), the folder beside it (subagent and tool output), and `file-history/<id>` and `session-env/<id>` (in `$CLAUDE_CONFIG_DIR` when that's set). It's gone from `claude --resume` too. The project's `memory/` folder isn't touched. Codex and OpenCode threads only leave the app.
  - While the window isn't focused, a thread that finishes, fails or asks for approval posts a system notification (clicking it opens the thread) and counts on the Dock icon's badge, which clears when you come back. Notifications need the bundled app (`cargo packager --release`); `cargo run` gets the badge only. Linux gets neither yet.
  - With the sidebar hidden, a badge on the sidebar button's corner says something wants you: amber for an approval, red for a failure, the accent colour for a finished turn.
- The toolbar starts with a "project › thread title" breadcrumb, as T3 Code's header does.
- Claude Code threads carry Claude's spark in its terracotta, the mark T3 Code uses (from its MIT-licensed `Icons.tsx`), instead of the OpenCode icon set's Anthropic "A".
- **Themes**: OpenCode, Nord and Gruvbox, each in light and dark (Settings › Appearance › Theme; Mode picks light, dark or the system's). Nord and Gruvbox keep the OpenCode kit's fonts, spacing and shadows and recolour it: every grey in OpenCode's palette maps by its lightness onto the theme's own ramp, and the accent, status and code colours come from the theme (`src/ui/theme.rs`).
- Settings › Agents counts the models each installed CLI reported. "Default" (let the CLI choose) isn't counted, and a CLI that isn't installed shows none.
- The sidebar also lists each project's saved Claude Code sessions (`~/.claude/projects/<folder>/*.jsonl`), from the terminal or earlier runs, titled as Claude Code titles them; opening one reads its transcript, and replying resumes it.
- Codex and OpenCode keep no history the app can read back, so Yoke saves those threads itself (`src/store.rs`) when each turn ends: they come back in their project at the next launch, and replying resumes the CLI's session. Deleting the thread deletes its file.
- **Worktree threads**, as in T3 Code: in a git repository the composer's Local / New worktree chip picks where a new thread runs. It starts at Settings › New threads › Runs in, and a change on the chip is for that thread only. In a worktree, the thread gets a branch of its own from the project's HEAD (`yoke/<first words of the prompt>-<id>`) in `~/.yoke/worktrees/<project>/`, so agents can change the same repository at once without touching each other's files or your checkout. The row shows the branch. Merging it back is up to you, and deleting the thread leaves the worktree and branch in place. Files git ignores (a local `.env`, `.cargo/config.toml`) aren't in a new worktree.
- **The composer floats over the conversation**, as in T3 Code: scroll up and the text shows faintly through its frosted card (a translucent fill over a backdrop blur), with the hint line under it on a fade of the pane's colour. At the bottom, the last message ends just above it.
- **Attachments.** Click the paperclip to pick a file, drop files anywhere on the window, or paste a screenshot into the prompt. Images (PNG, JPEG, GIF, WebP) go as images: Claude Code gets them as image blocks; Codex as `--image`; OpenCode gets the paths in the prompt. Any other file (or folder) goes by its path, listed under "Attached files:" at the end of the prompt for the agent to read; its contents aren't sent by Yoke.
- A transcript that streams Markdown as it arrives: code fences are syntax-coloured and tables, lists and inline code render. Text is selectable.
- Tool calls appear as rows: icon, tool name, a one-line summary and a running/ok/failed status. Click a row to open it. An edit shows as a diff (removed lines on red, added on green, changed words marked, and +added −removed on the row), a written file as code, a file read as the file beside its line numbers, and a command too long for its row in full, then its output. Code is coloured in the file's language (Rust, TypeScript and JavaScript, Python, Go, Swift, shell, JSON, SQL, HTML, CSS, YAML and diffs) in the theme's colours; `git diff` output is coloured as a diff. An edit's or write's "has been updated" is left out unless it failed.
- **Open in your editor**, as T3 Code's toolbar has: **Open** (⇧⌘O) opens the thread's folder (its worktree, if it has one) in the editor you picked last, and the menu beside it lists the editors, terminals and file manager found on this computer (Zed, Cursor, VS Code, Windsurf, Antigravity, Sublime Text, Xcode, Ghostty, iTerm, Terminal, Finder). On macOS they're found by their apps; elsewhere by their commands on PATH. The choice is saved in prefs. A read, edit or write row has its own ↗ under the pointer: it opens that file in the same editor (or the first editor found, if a terminal is picked) at the line: a read's first line, or where an edit's new text is in the file now. It uses the command each editor bundles in its app (`zed file:line`, `code -g file:line` and the same for Cursor, Windsurf and Antigravity, `subl file:line`, `xed -l line file`), so nothing has to be installed on PATH; VS Code and its forks also take their `vscode://file/…` links. With Finder picked, it shows the file in its folder.
- A "Worked for 48s · model · $cost" line after each turn.
- Follow-up messages resume the same CLI session (`--resume`).

## Harnesses

| Harness | Command | Status |
| --- | --- | --- |
| Claude Code | `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --permission-prompt-tool stdio …` | Tested live: streaming, resume, tool calls, stop. Approvals tested against a stand-in CLI; live test written (`live_claude_asks…`) |
| Codex | `codex exec --json …` | Parser written to the JSONL format; untested (not installed here) |
| OpenCode | `opencode run --format json …` | Parser written to the JSON event format; untested |

Harnesses not found on `PATH` (or in `~/.local/bin`, Homebrew and similar locations) are listed under "Not installed".
Each one is a `command` plus a `parse` function in `src/harness/`.

Model lists come from the CLIs themselves (`src/harness/models.rs`). For Claude Code, the app sends the Agent SDK's `initialize` control request (the one behind `supportedModels()`) to `claude -p --input-format stream-json` and reads back the models your account has. New models such as Fable appear without an app change, and no tokens are spent. OpenCode's list comes from `opencode models`. Codex can't list its models, so it keeps a short built-in list. The last answer is saved, so the list is right on the next launch before the CLI replies. If asking fails, the built-in list stays.

Steering stops the turn and resumes the session with the new message, because print mode (`-p`) can't take input once a turn has started. Claude Code's streaming input mode might accept a message mid-turn without the restart. That's untested, so it isn't used yet.

## Layout

```
src/main.rs            app spec, poll, scenes, live tests
src/bench.rs           perf benches (CPU, allocations and heap per frame)
src/app.rs             state: threads, items, the queue, applying harness events
src/shortcuts.rs       the shortcut table: commands, defaults, overrides, recording
src/harness/           spawning CLIs and parsing their JSON lines
src/harness/models.rs  asking each CLI for its models
src/harness/limits.rs  Claude's plan limits (`get_usage`, `rate_limit_event`)
src/usage.rs           spend from Claude Code's transcripts, priced from LiteLLM's table
src/history.rs         Claude Code's saved sessions, and the folders they ran in
src/store.rs           Codex and OpenCode threads (and worktree threads) the app saves itself
src/worktree.rs        making a git worktree for a thread
src/ui/mod.rs          window: SplitView shell, toolbar, new-thread hero
src/ui/sidebar.rs      thread list: inbox, Working and Settled shelves
src/attention.rs       notifications, Dock badge, auto-settle sweep
src/ui/theme.rs        OpenCode, Nord and Gruvbox kits
src/ui/transcript.rs   virtualised, memoised transcript
src/ui/composer.rs     prompt field and pickers
src/picker.rs          the model picker's state: rail, search, favorites, keys
src/ui/picker.rs       the model picker's chip and popover
src/ui/usage.rs        the Usage page and the two gauges (context and plan)
src/ui/hud.rs          frame stats (⇧⌘P)
```

## Checking it without a window

Rune runs any app headless, using the same renderer:

```sh
cargo build
target/debug/yoke --snap out.png --scene demo --size 1180x780 --theme light   # scenes: demo long empty picker effort settings usage queue approval attach collapsed
scripts/shots.sh                                              # re-renders shots/ (a scratch HOME, so none of your threads show)
cargo test                                                    # parsers, shortcuts, queue/steer and approvals (against a stand-in claude)
cargo test -- --ignored live_claude --nocapture               # drives the real `claude` through the UI (costs a few tokens)
cargo test -- --ignored live_usage --nocapture                # reads the real plan limits and transcripts (no tokens)
cargo test --profile perf -- --ignored bench --nocapture --test-threads=1   # perf numbers
```

## Performance (Apple silicon, `--profile perf`, headless at 2×)

The benches in `src/bench.rs` time each frame's CPU work: update, view, layout and paint. They don't cover GPU encode, presentation, or the window's drawables (about 42 MB at 1180×780). A counting allocator, used only in test builds, reports allocations and bytes per frame, along with heap growth. The frame-stats panel (⇧⌘P) shows the same frame times live.

| | p50 | p95 | max | allocs/frame |
| --- | --- | --- | --- | --- |
| Open a 3,000-row thread (Markdown, code, tables, tool rows) | 4.7 ms (one frame) | | | 45k |
| A frame that streams one text delta into it | 0.70 ms | 0.80 ms | 1.35 ms | 10k |
| A wheel-scroll frame through the history | 0.14 ms | 0.38 ms | 0.58 ms | 1 |
| An idle frame | 0.13 ms | | | 0 |
| A delta into a 0–8 KB answer | 0.57 ms | 0.78 ms | 1.45 ms | 2.9k |
| A delta into a 56–64 KB answer | 2.95 ms | 3.25 ms | 3.66 ms | 16k |
| 16 threads streaming at once, one on screen | 1.10 ms | 1.54 ms | 2.02 ms | 6.8k |

- Per-delta cost grows with the length of the streaming message. Rune's Markdown parses only the last block again, but each frame still rebuilds and lays out every block, so cost grows linearly. A 64 KB answer still fits a 120 Hz frame; a 250 KB one wouldn't.
- Thread state costs about 280 bytes per row. Tool output is kept whole, though a card shows only its first 40 lines, so a 1 MB `cargo test` log keeps about 1 MB alive.

Profiling the live app, or a bench, with function names (the `perf` profile keeps line tables):

```sh
cargo build --profile perf
samply record ./target/perf/yoke                    # macOS and Linux; `cargo install samply`
xcrun xctrace record --template 'Time Profiler' --launch -- ./target/perf/yoke   # macOS
perf record -g ./target/perf/yoke                   # Linux; `perf stat` for cache misses and IPC
footprint $(pgrep -n yoke)                          # macOS memory by kind
top -pid $(pgrep -n yoke) -stats cpu,mem,idlew,power   # idle wakeups and energy
```

Live with Claude Code (haiku), the first streamed text lands about 1.5 s after Return; that time is the CLI, not the UI.
The release build's native window was ready 663 ms after the process started.
At rest the window draws nothing: `poll` asks for a redraw only when an event lands, and once a second for the "Working…" counter.

What makes it fast is mostly Rune's own machinery:

- `virtual_rows` builds only the rows near the viewport.
- `memo` keeps each finished message's subtree, so a streaming delta rebuilds only the message that is growing.
- Scrolling never re-runs the view.

## Notes on Rune from building this

- The `rune-kit` shell (`SplitView`, `Sidebar`, `Toolbar`), `Markdown` and `Select` gave a polished T3/OpenCode look with very little code. The opencode theme crate brings the fonts, icons and tokens.
- The headless `--snap` and `Runtime` test harness made it possible to check layout and real agent turns without opening a window.
- `rune::Keybind` and `rune::Chord` cover the whole shortcut table: parsing "mod+shift+b", matching key events (with the user's layout), and labels such as "⇧⌘B". `rune::prefs::Prefs` saves the table. rune-kit's `ShortcutRecorder` is built for global hotkeys, so in-app rebinding uses a small recorder of its own that asks `Chord::matches` which chord a key press is.
- Gotchas hit:
  - A focused text field swallows Escape before the window's `on_key`, so Stop goes through the field's `on_input_key`.
  - `Select` menus size to their 160 pt minimum and truncate longer option labels.
  - A `memo` replaces its root's key.
  - The Window menu isn't registered as `NSApp.windowsMenu`, so macOS adds no tiling items and Fn-Control-arrows did nothing. Custom app-menu items also go above About. `src/ui/mac.rs` fixes both. They are worth fixing upstream.
  - A headless `Runtime` doesn't take the spec's `on_key`; tests set `rt.on_key` themselves.
  - `Toolbar`'s band is a fixed 48 points (`TOOLBAR_HEIGHT`). With `SplitView::toolbar_in_panel` and an 8-point frame inset it centres 32 points down, and SplitView's sidebar toggle with it, about 8 points lower than other macOS apps put the traffic lights. Moving the toolbar out of the card fixes the line but puts a bar over the card. So the toolbar stays in the card, its contents are lifted 8 points (`El::offset`), and the app draws its own sidebar toggle on the lights' line (`ui::sidebar_toggle`), with SplitView's hidden. A settable band height, or a toolbar offset, upstream would remove both workarounds.
  - Kit colours are linear: `Color` stores linear sRGB, so `{:?}` of a palette doesn't show the hex you'd see. `Color::hex()` and `rune::color::hex()` convert.
  - `SidebarRow` centres its detail and status slot on the whole row, and `on_close` reserves the 20-point slot on every row. For mail-style rows (time on the first line, status on the second) the thread rows build their own `trailing` column and hover actions with `group_opacity`.
  - `SidebarSection::collapsed` drops every row, so the Settled shelf adds the open thread's row itself when it's shut.
  - A row that is still springing to a new place doesn't take the hover yet. In tests, let the motion finish before hovering.
  - A `Popover` settles beside its trigger over the frames after it opens. A test that presses something inside it on the very next frame hits the scrim (and closes it), though `element_at` already reports the button.
  - There's no `flex-basis`. Equal-width tiles and chart columns are `.w(0.0).grow(1.0)`.
  - An `Icon`'s view is square, so a non-square SVG (Claude's spark is 256 × 257) takes the larger side. A monochrome icon tinted with `.color()` keeps a brand colour without Rune's `svg` feature.
  - `--snap` runs `setup`, so background reads it starts could land in a snapshot. The usage scenes hold them off by putting channels with no sender where the running reads' channels go.
