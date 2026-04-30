# Contributing to acrawl

Thanks for your interest in contributing. This document covers the basics.

## Reporting Bugs

Open a [bug report](https://github.com/Mingye-Lu/AgenticCrawler/issues/new?template=bug_report.yml). Include steps to reproduce, expected vs actual behavior, and your environment (OS, Rust version, acrawl version).

## Suggesting Features

Open a [feature request](https://github.com/Mingye-Lu/AgenticCrawler/issues/new?template=feature_request.yml) describing the problem you're trying to solve and your proposed solution.

## Development Setup

```bash
git clone https://github.com/Mingye-Lu/AgenticCrawler.git
cd AgenticCrawler
cargo build --release

# Browser automation requires Playwright's Chromium
npm install
npx playwright install chromium
```

## Code Style

All code must pass these checks before merge:

```bash
cargo fmt --check                                         # formatting
cargo clippy --workspace --all-targets -- -D warnings     # lints (pedantic)
cargo test --workspace                                    # tests
```

- **No unsafe code.** The workspace sets `unsafe_code = "forbid"`.
- Clippy pedantic is enabled as a warning. Fix new warnings rather than suppressing them.
- If your test mutates process-level state (env vars, global config), serialize it with the `OnceLock<Mutex<()>>` guard pattern used elsewhere in the codebase. See `crates/runtime/src/lib.rs::test_env_lock` for the reference implementation.

## Submitting a Pull Request

1. Fork the repo and create a branch from `main`.
2. Make your changes. Keep commits focused — one logical change per commit.
3. Use [conventional commit](https://www.conventionalcommits.org/) messages: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
4. Ensure CI passes (fmt, clippy, test, build).
5. Open a PR against `main`. Fill in the PR template.

Small, well-scoped PRs are reviewed faster than large ones.

## Project Structure

```
crates/
  acrawl-cli/   CLI binary, TUI REPL, session management
  api/          LLM provider clients (Anthropic, OpenAI, Codex)
  commands/     Slash command registry
  crawler/      Browser tools, agent loop, Playwright bridge
  runtime/      ConversationRuntime, config, permissions, sessions
```

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
