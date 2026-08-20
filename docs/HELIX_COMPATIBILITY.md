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
- File picker on `Space f`, in-place current-buffer fuzzy search on `/`,
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

## Next audit groups

- Remaining tutor editing operators: find/till, replace, yank/paste, join,
  indent/unindent, case changes, comments, selection splitting, and alignment.
- Registers, macros, repeat-last-motion/change, count prefixes, diagnostics,
  symbol/code-action commands, and clipboard integration.
- View prefix behavior, buffer/jump pickers, split/window commands, and picker
  history.
- Search parity for reverse search and fuzzy occurrence splitting beyond exact
  in-selection occurrences.
