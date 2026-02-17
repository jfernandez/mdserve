# Contributing to mdserve

Thank you for your interest in contributing to mdserve!

## Commit Message Format

We follow the [Conventional Commits](https://www.conventionalcommits.org/) specification. All commit messages must follow this format:

```
type(optional-scope): description

[optional body]

[optional footer(s)]
```

### Type

Must be one of the following:

- **feat**: A new feature
- **fix**: A bug fix
- **docs**: Documentation only changes
- **style**: Changes that do not affect the meaning of the code (white-space, formatting, etc)
- **refactor**: A code change that neither fixes a bug nor adds a feature
- **perf**: A code change that improves performance
- **test**: Adding missing tests or correcting existing tests
- **chore**: Changes to the build process or auxiliary tools and libraries
- **ci**: Changes to CI configuration files and scripts
- **revert**: Reverts a previous commit

### Scope

Optional. Can be anything specifying the place of the commit change (e.g., `deps`, `cli`, `server`).

### Examples

```
feat: add support for subdirectory images
fix: resolve websocket connection timeout
docs: update installation instructions
chore(deps): update axum to 0.7
ci: add commitlint workflow
```

### Subject

- Use imperative, present tense: "add" not "added" nor "adds"
- Don't capitalize the first letter
- No period (.) at the end
- Maximum 72 characters

## Pull Request Process

1. Fork the repository and create your branch from `main`
2. Follow the commit message format above for all commits
3. Update documentation if needed
4. Ensure CI passes (tests, clippy, formatting, commit messages)
5. Submit your pull request

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and fix any warnings
- Follow existing code conventions

### Pre-commit Hooks (Recommended)

We use [prek](https://github.com/j178/prek) to run automated checks before each commit. This ensures code quality and catches issues early.

**Installation:**

```bash
# Using curl (macOS/Linux)
curl -fsSL https://prek.j178.dev/install.sh | sh

# Using pipx
pipx install prek

# Using cargo
cargo install prek
```

**Setup:**

```bash
# Install the git hooks
prek install
```

Once installed, prek will automatically run on each commit to check:
- Code formatting (`cargo fmt`)
- Linter warnings (`cargo clippy`)
- Tests (`cargo test`)

**Manual run:**

```bash
# Run all hooks
prek run --all-files

# Run specific hook
prek run cargo-fmt
```

## Questions?

Feel free to open an issue for any questions about contributing.
