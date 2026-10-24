# Contributing

Thanks for your interest in vEdit. It is a small project with one maintainer,
so please open an issue before you start on a larger change. Bug reports with
clear steps to reproduce help the most.

## Building and testing

vEdit needs Windows and the stable Rust toolchain.

```powershell
cargo build --release
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

[docs/development.md](docs/development.md) covers targeted tests, visual
snapshot references, optional language-server checks and packaging.
[AGENTS.md](AGENTS.md) holds the repository conventions that coding agents and
people follow alike.

## Pull requests

- Keep one logical change per pull request and add or extend a test for the
  behavior you change.
- Write commit messages in the Conventional format, for example
  `fix(git): keep parent repository after restart`.
- Update the user manual [docs/handbuch.md](docs/handbuch.md) (German) when
  user-visible behavior changes. If German is a hurdle, say so in the pull
  request and the maintainer will take care of it.

By contributing, you agree that your contribution is licensed under the
[MIT License](LICENSE).
