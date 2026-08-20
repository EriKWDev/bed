# Helix compatibility inventory

Bed uses Helix's interaction model as its behavioral reference, without sharing
its implementation. Audit behavior against these files in the adjacent Helix
checkout:

- `../helix/runtime/tutor` for taught editing behavior and expected workflows.
- `../helix/helix-term/src/keymap/default.rs` for the complete default normal,
  select, insert, goto, match, view, and Space keymaps.
- `../helix/helix-term/src/commands/typed.rs` for command names, aliases,
  argument signatures, and completion domains.

Review all three whenever bindings or commands change. The tutor is the minimum
interactive behavior, while the keymap and typed-command tables are the
exhaustive inventories.

## Implemented baseline

- Selection-first normal mode, explicit extending select mode, insert mode,
  line selection, word movement, line boundaries, goto prefix, match objects,
  multiple cursors, insertion on adjacent lines, undo/redo transactions, and
  jump-list traversal.
- Clean adjacent-line insertion with automatic indentation cleanup, default-on
  bracket/quote pairing, paired-scope Enter indentation, and non-moving blank
  lines on `[Space`/`]Space`. Collapsed insert sessions remain collapsed and
  Backspace removes untouched paired closers.
- File picker on `Space f`, in-place contiguous smart-case search on `/`,
  project-wide fuzzy content search on `Space /`, next/previous accepted search
  results on `n`/`N`, and in-selection occurrence search on `s`.
- Command palette with command-token completion and argument-aware theme/file
  completion. File completion only substitutes on Tab; Enter preserves literal
  paths.
- Scratch buffer creation; current, other, and all-buffer closing with force
  variants; next/previous buffer traversal; write-all, quit-all, and
  write-quit-all aliases.
- Theme command and five built-in color themes, with Kanagawa as the default and
  the configured `Space t` shortcuts.
- Bottom-right continuation hints for every implemented multi-key prefix.
- One-based `g<number>g`, `gg`, and previous/next Git-change traversal on
  `[g`/`]g`, backed by a nonblocking minimal Git gutter.
- Extension-selected resumable syntax highlighting for TOML, Markdown, Rust,
  C, C++, Go, Nim, and Odin, including structured comment annotations and
  separate control/declaration/constant/operator classes; local Rust `gd`
  definition lookup and `gr` reference selection over the
  incomplete-source-tolerant flat symbol index.
- Document/workspace symbol palettes on `Space s`/`Space S` and explicit-type
  Rust member completion using an asynchronously built compact method index for
  workspace and active-toolchain standard-library sources.
- Internal multi-value yank/paste on `y`, `p`, and `P`; asynchronous system
  clipboard yank/paste on `Space Y`/`Space P`; undoable `:reload` from disk.
- Helix-style word/punctuation/whitespace boundaries for `w` and `b`, module
  file and workspace-symbol definition fallback, whole-line global-search
  acceptance, and identifier-selecting symbol palette acceptance.

## Next audit groups

- Remaining tutor editing operators: find/till, replace, yank/paste, join,
  indent/unindent, case changes, comments, selection splitting, and alignment.
- Registers, macros, repeat-last-motion/change, count prefixes, diagnostics,
  symbol/code-action commands, and clipboard integration.
- View prefix behavior, buffer/jump pickers, split/window commands, and picker
  history.
- Search parity for reverse search and fuzzy occurrence splitting beyond exact
  in-selection occurrences.
