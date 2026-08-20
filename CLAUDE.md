# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build          # compile
cargo run            # build and run
cargo test           # run all tests
cargo test <name>    # run a single test by name filter
cargo clippy         # lint
cargo fmt            # format
```

## Project

`bed` is a lightweight, high-performance terminal text editor in Rust with Helix/kakoune-style keybindings. Rust 2024 edition, single binary.

### Design constraints

- **No LSP, no tree-sitter, no web tech.** All code intelligence is custom and index-based.
- **No fancy tree structures.** No ropes, no B-trees as primary text or index structures. Prefer flat arrays, sorted vecs, and gap buffers — contiguous memory wins.
- All data structures must be cache-friendly and allocation-minimal.

### Core subsystems (planned)

| Subsystem | Approach |
|-----------|----------|
| Text storage | Gap buffer — single contiguous allocation with a movable gap |
| Terminal I/O | Raw termios directly; no ncurses/crossterm unless weight is justified |
| Fuzzy search | In-process, score-ranked over flat byte arrays |
| Rust code intelligence | Custom source scanner: builds flat symbol/scope tables by parsing `.rs` files without LSP. Supports go-to-definition, go-to-references, symbol lookup, rename. |
| Keybindings | Helix/kakoune modal model: selections-first, multi-cursor |

### Rust code intelligence design

The goal is a "best-effort" static index, not full type inference:
- Walk workspace source files, extract item definitions (fns, structs, enums, traits, impls, consts, macros) with byte offsets.
- Store in sorted flat tables (file × offset → symbol, name → locations).
- Rename: text-substitute across all files using the index — no semantic correctness guarantee on complex cases, but correct for the common case.
- No JSON-RPC, no child processes, no dynamic plugins.
