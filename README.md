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
| normal | `h/j/k/l`, Helix-category `w/b`, or arrows move selections; `x` selects/extends whole lines; `d` deletes; `y` yanks and `p`/`P` paste after/before; `i/a/I/A/o` enter insert; `v` extends selections; `[Space`/`]Space` insert a blank line without moving |
| select | movement extends the selection, `d` deletes it, `v` or Escape returns to normal |
| insert | text inserts, Backspace/Delete edit, arrows move, Escape returns to normal |
| objects | `ma<char>` selects around a matching pair; `mi<char>` selects inside it; `maf`/`mif` select the outermost enclosing function; all remain normal-mode motions |
| multiple cursors | `C` adds a cursor on the line below; movement and editing affect every selection |
| search | `/` performs contiguous smart-case search in the current buffer from the cursor forward with wraparound; `s` searches within the current selections and previews one cursor per occurrence; `n`/`N` traverse a committed `/` search |
| code navigation | in Rust buffers, `gd` selects the nearest definition and `gr` opens a reference picker; `[f`/`]f` select functions, `[g`/`]g` visit Git changes, and `[d`/`]d` visit diagnostics |
| pickers | `Space f` searches project files; `Space /` searches contents; `Space s`/`Space S` search symbols; `Space d`/`Space D` show document/workspace diagnostics; `:` opens the command palette |
| history | `u`/`U` undo/redo; Ctrl-O/Tab move backward/forward through jumps |
| any | Ctrl-S saves, Ctrl-Q discards changes and quits |

Every implemented multi-key prefix displays a bottom-right continuation panel
with its available keys and descriptions. This currently covers `Space`,
`Space t`, `g`, `m`, `ma`, and `mi`; the panel table is extended alongside new
prefix bindings so it never advertises unavailable commands. `g<number>g`
jumps to a one-based line number, while `gg` retains its file-start behavior.
`Space Y` copies selections to the system clipboard and `Space P` pastes from
it; both external operations run on the worker pool rather than the input path.

The command palette completes command names first and switches completion to
the command's arguments after a space. `:theme ` and `:t ` complete theme
names; file-taking commands such as `:write `, `:w `, `:open `, and `:o `
complete project-relative paths with Tab while Enter keeps the typed path
verbatim. Buffer commands include `:new`/`:n`, `:bc[!]`, `:bco[!]`,
`:bca[!]`, `:bn`, and `:bp`. The write/quit families include `:wa[!]`,
`:qa[!]`, and `:wqa[!]`. `:rl[!]` reloads the current buffer and
`:rla[!]` reloads every file-backed buffer through undoable replacements.

Kanagawa is the default theme. `:theme <name>` selects `kanagawa`, `bogster`,
`everforest_light`, `noctis`, or `kaolin-valley-dark`. The same set is on
`Space t d/k/b/l/n/v` respectively. Selection, normal cursor, secondary insert
cursor, gutter, status line, picker, Git indicators, foreground, and background colors are
theme-controlled; the primary insert cursor remains a terminal bar with a
theme-controlled cursor color. During `s`, the original selection is rendered
as a faint search scope while occurrences use the stronger selection color and
their cursors use the cursor color.

Extension-selected syntax highlighting currently covers TOML, Markdown, Rust,
C, C++, Go, Nim, and Odin. It uses a resumable byte scanner and retained flat
span storage: opening a file gets a one-millisecond immediate pass, then any
remaining work continues in 500-microsecond event-loop slices. Edits clear and
reuse the span allocation. No parser task, syntax tree, or tree-sitter runtime
sits between input and rendering. Themes distinguish comments, structured
comment annotations (`NOTE`/`HELP` in blue, `WARNING`/`HACK` in orange,
`FIXME`/`ERROR` in emphatic red, and other uppercase labels), Rust lifetimes,
ordinary/control/declaration keywords, strings, numbers, types,
functions, constants, attributes, operators, punctuation, and markup.

Rust buffers also maintain a resumable flat identifier/definition index. It
ignores comments and strings, prefers the nearest definition visible at the
identifier's lexical scope, and deliberately accepts incomplete source. This
also backs document symbol lookup. Workspace symbol lookup includes top-level
workspace and active-toolchain standard-library symbols. Explicit local types support member completion: for
example `potato: Vec<usize>` resolves `potato.pu` against a compact method index
built from workspace Rust sources and the active toolchain's installed
standard-library sources. Toolchain/sysroot discovery and source reads run on
the shared worker pool; typing only performs local inference and ranks retained
method spans. Tab accepts a completion and Ctrl-N/Ctrl-P changes the selected
candidate. This remains intentionally tolerant inference rather than a compiler
front end.
`gd` resolves constants, statics, `mod name;` files, top-level workspace
symbols, standard-library types, and methods on explicitly typed receivers.
Definitions and picker results select the identifier with the cursor at its
start. Cross-file navigation records only the origin and final selection, so
Ctrl-O returns directly to the call site. `gr` shows exact retained workspace
occurrences in a picker rather than turning them into editing cursors.

Cargo diagnostics are collected on the worker pool and published as one
severity-sorted retained snapshot. `Space d`/`Space D` show file/workspace
diagnostic pickers and `[d`/`]d` navigate them with errors before warnings and
informational messages. Saving requests a refresh without blocking input.

Opening a directory starts the file picker. File discovery loads exactly one
authoritative ignore file, choosing the first existing file in this order:
`.bedignore`, `.editorignore`, `.ignore`, `.gitignore`. Matching and fuzzy
ranking are local byte-oriented implementations; the binary does not pull in a
filesystem walker, regex engine, or fuzzy-search library.
Opening a specific file uses its canonical parent as the project root, so a
later `Space f` sees the same corpus as opening that parent directory first.

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

Insert mode auto-pairs `()`, `[]`, `{}`, and double quotes by default. Enter
between brackets creates an indented interior line and a dedented closing line.
Unused automatic indentation is removed on Escape, while the entire insert
session remains one undo transaction. `:toggle-auto-pairs`,
`:toggle-auto-indentation`, and `:toggle-auto-indent-scopes` control these
behaviors.
Collapsed cursors remain collapsed through `i` and `o`; only a genuinely
extended pre-existing selection expands or retracts with insert edits. Deleting
an untouched auto-inserted opener with Backspace also deletes its paired closer.
Within leading space indentation, Backspace removes a full configured unit only
on exact unit boundaries; partial indentation deletes one space.
`:reload`/`:rl` and `:reload-all`/`:rla` reread from disk through undoable
replacements.

Git gutter baselines are fetched off the input thread and current-buffer line
hashing/diffing proceeds in bounded event-loop slices. Additions and changes use
minimal vertical gutter marks; removals use a red boundary mark. Rendering uses
synchronized terminal updates when supported and does not clear the whole
screen per movement. Terminal size is polled while idle so resizes redraw
without requiring another keypress.
Visible syntax spans and Git markers are retained while replacements are
rebuilt, remapped immediately across edits, and atomically replaced when the
bounded rebuild completes. Normal document rendering hashes logical rows and
transmits only changed rows plus the status line, so cursor movement no longer
rewrites the entire viewport.
Touched Git lines keep a provisional modified marker until the completed diff
proves they match the baseline again. Vertical motion retains its preferred
column across short lines and restores it on longer lines.

The current core deliberately has no platform abstraction, LSP, tree-sitter,
terminal framework, or rope. Future editing operations should continue to work
over byte offsets and flat selections, with Unicode boundaries handled at the
edges where motions and input require them.

The Helix compatibility inventory and the upstream resources used to audit it
are recorded in [docs/HELIX_COMPATIBILITY.md](docs/HELIX_COMPATIBILITY.md).
