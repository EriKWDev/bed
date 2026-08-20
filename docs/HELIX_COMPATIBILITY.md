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
  lines on `[Space`/`]Space`. Repeated Enter leaves intermediate lines empty,
  collapsed insert sessions remain visually collapsed, configured Tab inserts
  spaces, and Backspace removes untouched paired closers.
- File picker on `Space f`, in-place contiguous smart-case search on `/`,
  project-wide fuzzy content search on `Space /`, next/previous accepted search
  results on `n`/`N`, and in-selection occurrence search on `s`.
- Command palette with command-token completion and argument-aware theme/file
  completion. File completion only substitutes on Tab; Enter preserves literal
  paths.
- Window-proportional bordered picker dialogs with Tab/Shift-Tab navigation and reusable,
  syntax-highlighted split previews for file, symbol, reference, search, and
  diagnostic locations. Preview target lines keep syntax foregrounds over the
  focus background, and toolchain symbols load bounded source windows away
  from the input thread when they are outside the workspace search corpus.
- Scratch buffer creation; current, other, and all-buffer closing with force
  variants; next/previous buffer traversal; write-all, quit-all, and
  write-quit-all aliases.
- Theme command and five built-in color themes, with Kanagawa as the default and
  the configured `Space t` shortcuts.
- Bottom-right continuation hints for every implemented multi-key prefix.
- One-based `g<number>g`, `gg`, and previous/next Git-change traversal on
  `[g`/`]g`, backed by a nonblocking minimal Git gutter.
- Extension-selected resumable syntax highlighting for TOML, Markdown, Rust,
  C, C++, Go, Nim, Odin, and Bash/Zsh shell files, including structured comment annotations and
  severity colors, Rust lifetimes, and separate control/declaration/constant/operator classes; local Rust `gd`
  definition lookup and a `gr` reference picker over the
  incomplete-source-tolerant flat symbol index.
- Syntax-preserving selection backgrounds and Helix-aligned TOML table-key,
  pair-key, boolean, punctuation, date/time, string, number, and comment scopes.
- Picker previews keep target-line context subdued while applying the stronger
  selection background only to the exact symbol/reference span.
- Document/workspace symbol palettes on `Space s`/`Space S` and explicit-type
  Rust member completion using an asynchronously built compact method index for
  workspace, Cargo dependency, and active-toolchain standard-library sources.
  Cursor-anchored completion also includes visible locals/arguments, keywords,
  functions, and types; Tab/Shift-Tab, arrows, and Ctrl-N/Ctrl-P only cycle,
  while Enter accepts. Documentation renders in a separate dark panel.
  Module navigation is deliberately declaration-first, then file-opening on a
  second `gd`; type aliases are similarly declaration-first and then follow
  their targets. Primitive types select the toolchain's canonical `prim_*`
  documentation declaration. An explicit lookup miss joins all outstanding
  Rust indexing before it becomes a definitive failure. Completion details
  retain and display method declarations and adjacent doc comments.
- Internal multi-value yank/paste on `y`, `p`, and `P`; asynchronous system
  clipboard yank/paste/selection replacement on `Space Y`/`Space P`/`Space R`;
  undoable `:reload` from disk. `Ctrl-C` toggles correctly indented line
  comments across all lines touched by all selections.
- Helix-style word/punctuation/whitespace boundaries for `w` and `b`, module
  file and workspace-symbol definition fallback, whole-line global-search
  acceptance, and identifier-selecting symbol palette acceptance. Rust
  reference candidates exclude non-Rust files; local bindings and fields are
  filtered by declaration identity and inferred owner rather than spelling.
  Workspace occurrences are retained name-sorted and binary-ranged, and an
  explicit `gr` joins unfinished discovery/index publication before answering.
- Preferred-column vertical motion; function objects on `maf`/`mif`; nested
  function traversal on `[f`/`]f`; standard-library symbol/method definitions;
  centered symbol targets, jump-list viewport restoration, atomic cross-file
  jumps, and undoable `:rl`/`:rla` reload aliases.
- Nonblocking Cargo diagnostics with severity-ordered document/workspace
  pickers on `Space d`/`Space D` and navigation on `[d`/`]d`.
- Surround add/replace on `ms<char>` and `mr<old><new>`, plus nonblocking
  undoable `:format` for the built-in Rust, C/C++, and Go formatter commands.

## Next audit groups

- Remaining tutor editing operators: find/till, replace, yank/paste, join,
  indent/unindent, case changes, comments, selection splitting, and alignment.
- Registers, macros, repeat-last-motion/change, remaining count prefixes,
  richer code actions, and semantic inference beyond explicit local types.
- View prefix behavior, buffer/jump pickers, split/window commands, and picker
  history.
- Search parity for reverse search and fuzzy occurrence splitting beyond exact
  in-selection occurrences.
