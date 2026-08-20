#![allow(clippy::question_mark)]
#![feature(allocator_api)]

mod buffer;
mod code_index;
mod diagnostics;
mod editor;
mod fuzzy;
mod git;
mod project;
mod rust_methods;
mod syntax;
mod terminal;

#[global_allocator]
static PROCESS_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    if let Err(error) = run() {
        eprintln!("bed: {error}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let mut editor = match editor::editor_open(path) {
        Ok(editor) => editor,
        Err(error) => return Err(error),
    };
    let mut terminal = match terminal::terminal_open() {
        Ok(terminal) => terminal,
        Err(error) => return Err(error),
    };

    return editor::editor_run(&mut editor, &mut terminal);
}
