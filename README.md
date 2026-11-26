# GitHub Actions (with repository context) for Zed

A [Zed](https://zed.dev) extension for GitHub Actions workflows. It runs GitHub's official language server ([`@actions/languageserver`](https://github.com/actions/languageservices)), the same one behind the VS Code extension, and hands it the repository context it needs to talk to github.com. With that, a `secrets.X` or `vars.X` that does not exist is underlined with `Context access might be invalid: X`, checked against:

- the repository and organization secrets and variables;
- the job's `environment:`, when the job has one.

You also get completion, hover and schema validation for workflows.

## How the context is detected

When a workflow is opened, the extension:

1. takes the token from `GH_TOKEN`/`GITHUB_TOKEN` in your shell or, failing that, from `gh auth token`;
2. reads the `origin` remote with `git` to find the `owner/name`;
3. looks up the repository id and whether it belongs to an organization with `gh api repos/<owner>/<name>`.

Without a token or a github.com remote the server still runs, just without the validations that need github.com. The token needs the `repo` and `workflow` scopes (the default `gh auth login` has them).

## Installation

1. Install the [GitHub CLI](https://cli.github.com) and log in with `gh auth login`.
2. Clone this repository and, in Zed, run `zed: install dev extension` from the command palette, picking the cloned folder. Zed builds the extension, which needs Rust installed through `rustup`.
3. Associate workflows with the `GitHub Actions` language in your Zed `settings.json`, so other YAML files stay YAML:

   ```jsonc
   "file_types": {
     "GitHub Actions": ["**/.github/workflows/*.yml", "**/.github/workflows/*.yaml"]
   }
   ```

4. Open a workflow. The first time, the extension installs `@actions/languageserver` through npm.

After creating a secret or variable on GitHub, run `editor: restart language server` so the server fetches the lists again.

## Optional settings

Values in `lsp.actions-languageserver.initialization_options` override the detected ones, e.g. to use another token or turn on logging:

```jsonc
"lsp": {
  "actions-languageserver": {
    "initialization_options": { "logLevel": 0 }
  }
}
```

## Limitation

The official server skips validation for jobs whose environment is an expression, like `environment: production-${{ matrix.cell }}` (VS Code behaves the same). For those, [`scripts/gh-check-env`](scripts/gh-check-env) checks every workflow from the command line, expanding the `matrix`:

```bash
cd my-repository && path/to/gh-check-env
```

## Development

```bash
cargo test                                     # unit tests
cargo build --release --target wasm32-wasip2   # the same build Zed runs
```

Run Zed with `zed --foreground` or use `dev: open language server logs` to see the extension and server logs.

## License

GPL-3.0-or-later. The Tree-sitter queries in `languages/github-actions/` come from Zed's built-in YAML support, which is licensed under the GPL.
