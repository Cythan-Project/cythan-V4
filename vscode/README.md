# Cythan VS Code extension

LSP-backed editor support for `.ct` files: live diagnostics from the
parser / typer / HIR generator, syntax highlighting, and basic
language configuration (brackets, comments, indentation).

## Layout

```
vscode/
├── package.json                       extension manifest
├── tsconfig.json                      TypeScript build config
├── language-configuration.json        brackets/comments/indentation
├── syntaxes/cythan.tmLanguage.json    TextMate grammar
└── src/extension.ts                   activates + spawns the LSP
```

The language server itself lives in
[`crates/lsp/`](../crates/lsp/) and is built as the `cythan-lsp`
binary.

## Build

```bash
# 1. Build the language server.
cargo build --release -p cythan-lsp
# The binary lands at target/release/cythan-lsp.

# 2. Install the extension's TypeScript dependencies + compile.
cd vscode
npm install
npm run compile
```

## Run during development

1. Open this repo's root in VS Code.
2. Run "Run Extension" from the Run & Debug pane (or press F5).
3. In the spawned Extension Host window, open any `.ct` file
   (e.g. `examples/new_syntax/Morpion.ct`). Diagnostics appear in
   the Problems pane and inline.

If `cythan-lsp` isn't on `PATH`, set the binary location in
**Settings → Extensions → Cythan → Server Path**:

* `target/release/cythan-lsp` (relative to the workspace folder), or
* an absolute path like `/Users/me/code/cythan-V4/target/release/cythan-lsp`.

## Settings

| key                   | default                                       | meaning                                                                 |
|-----------------------|-----------------------------------------------|-------------------------------------------------------------------------|
| `cythan.serverPath`   | `cythan-lsp`                                  | Where to find the LSP binary                                            |
| `cythan.stdDir`       | `<workspace>/examples/new_syntax/std`         | Override the stdlib directory (every `.ct` file inside is auto-loaded)  |
| `cythan.trace.server` | `off`                                         | Trace JSON-RPC traffic for debugging the protocol                       |

## Capabilities (today)

* Push diagnostics on open / change / save / close, mapping
  `errors::Diagnostic` 1:1 to `lsp_types::Diagnostic` (severity,
  `DiagCode` as the LSP `code`, primary span as the diagnostic
  range, secondary spans as `relatedInformation`, notes/helps
  appended to the message).
* Multi-file analysis: the open document plus every `.ct` file
  in `cythan.stdDir` is fed to the typer so cross-file references
  resolve.

Hover, go-to-definition, completions etc. aren't implemented yet —
the underlying typer keeps spans and a symbol registry, so each
of these is a small additional handler.
