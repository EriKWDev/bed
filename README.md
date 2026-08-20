# bed

`bed` is an early terminal text editor with a Helix/Kakoune-style modal,
selection-first command model. It uses a contiguous gap buffer and talks to a
Unix terminal directly through termios and ANSI escape sequences.

```sh
cargo run -p bed -- path/to/file
```

Current bindings:

| mode | keys |
|---|---|
| normal | `h/j/k/l`, `w/b`, or arrows move selections; `x` selects/extends whole lines; `d` deletes; `i/a/I/A/o` enter insert; `v` extends selections |
| select | movement extends the selection, `d` deletes it, `v` or Escape returns to normal |
| insert | text inserts, Backspace/Delete edit, arrows move, Escape returns to normal |
| objects | `ma<char>` selects around a matching pair; `mi<char>` selects inside it |
| multiple cursors | `C` adds a cursor on the line below; movement and editing affect every selection |
| search | `/` smart-case fuzzy-searches and highlights the current buffer in place from the cursor forward with wraparound; `s` searches within the current selections and previews one cursor per occurrence; `n`/`N` traverse a committed `/` search |
| pickers | `Space f` searches project files; `Space /` fuzzy-searches all ignored-filtered project files; `:` opens the command palette |
| history | `u`/`U` undo/redo; Ctrl-O/Tab move backward/forward through jumps |
| any | Ctrl-S saves, Ctrl-Q discards changes and quits |

The command palette completes command names first and switches completion to
the command's arguments after a space. `:theme ` and `:t ` complete theme
names; file-taking commands such as `:write `, `:w `, `:open `, and `:o `
complete project-relative paths with Tab while Enter keeps the typed path
verbatim. Buffer commands include `:new`/`:n`, `:bc[!]`, `:bco[!]`,
`:bca[!]`, `:bn`, and `:bp`. The write/quit families include `:wa[!]`,
`:qa[!]`, and `:wqa[!]`.

Kanagawa is the default theme. `:theme <name>` selects `kanagawa`, `bogster`,
`everforest_light`, `noctis`, or `kaolin-valley-dark`. The same set is on
`Space t d/k/b/l/n/v` respectively. Selection, normal cursor, secondary insert
cursor, gutter, status line, picker, foreground, and background colors are
theme-controlled; the primary insert cursor remains a terminal bar with a
theme-controlled cursor color. During `s`, the original selection is rendered
as a faint search scope while occurrences use the stronger selection color and
their cursors use the cursor color.

Opening a directory starts the file picker. File discovery loads exactly one
authoritative ignore file, choosing the first existing file in this order:
`.bedignore`, `.editorignore`, `.ignore`, `.gitignore`. Matching and fuzzy
ranking are local byte-oriented implementations; the binary does not pull in a
filesystem walker, regex engine, or fuzzy-search library.

Small projects are discovered and content-indexed inline. Broad roots publish
an initial bounded file set immediately, then continue a breadth-first walk in
entry- and time-bounded event-loop batches. An open file picker addresses files
directly while its query is empty; with a query it retains all matches but only
merges and sorts a bounded top-ranked display window as new paths arrive.
Previously discovered paths are not rescored. Content indexing uses the
existing thread pool. Every nested `target`, `node_modules`, and `.git`
directory is excluded from broad traversal.
Global queries are persistent, budgeted ranking passes: extending a query
searches its existing candidates first, edits reuse already-found candidates,
and no keystroke spawns a new task. All matches remain accumulated while only
a small top-ranked window is sorted for display.

The current core deliberately has no platform abstraction, LSP, tree-sitter,
terminal framework, or rope. Future editing operations should continue to work
over byte offsets and flat selections, with Unicode boundaries handled at the
edges where motions and input require them.

The Helix compatibility inventory and the upstream resources used to audit it
are recorded in [docs/HELIX_COMPATIBILITY.md](docs/HELIX_COMPATIBILITY.md).
