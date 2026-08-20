use core::alloc::Allocator;

use crate::buffer::{GapBuffer, buffer_byte, buffer_len};
use crate::fuzzy::{FuzzyMatch, fuzzy_rank};

#[derive(Clone, Copy)]
pub struct RustMethod {
    pub owner_start: u32,
    pub owner_end: u32,
    pub name_start: u32,
    pub name_end: u32,
    pub path: u32,
    pub position: u32,
    pub end: u32,
}

#[derive(Clone, Copy)]
pub struct RustSymbol {
    pub name_start: u32,
    pub name_end: u32,
    pub path: u32,
    pub position: u32,
    pub end: u32,
}

pub struct RustMethodCorpus {
    pub bytes: Vec<u8>,
    pub methods: Vec<RustMethod>,
    pub symbols: Vec<RustSymbol>,
    pub paths: Vec<std::path::PathBuf>,
    pub standard_library_available: bool,
}

pub struct RustMethodIndex {
    pub corpus: RustMethodCorpus,
    pub task: Option<idno_std::micropool::OwnedTask<RustMethodCorpus>>,
}

pub fn rust_method_index_empty() -> RustMethodIndex {
    RustMethodIndex {
        corpus: RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: false,
        },
        task: None,
    }
}

pub fn rust_method_index_start(
    root: &std::path::Path,
    project_paths: std::sync::Arc<Vec<std::path::PathBuf>>,
) -> RustMethodIndex {
    profiling::function_scope!();
    let root = root.to_path_buf();
    let task =
        idno_std::threads().spawn_owned(move || rust_method_corpus_build(&root, &project_paths));
    let mut index = rust_method_index_empty();
    index.task = Some(task);
    index
}

pub fn rust_method_index_poll(index: &mut RustMethodIndex) -> bool {
    profiling::function_scope!();
    let complete = index
        .task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let Some(task) = index.task.take() else {
        return false;
    };
    match task.try_join() {
        Ok(corpus) => index.corpus = corpus,
        Err(task) => {
            index.task = Some(task);
            return false;
        }
    }
    true
}

pub fn rust_method_index_pending(index: &RustMethodIndex) -> bool {
    index.task.is_some()
}

pub fn rust_method_name(corpus: &RustMethodCorpus, method: usize) -> &str {
    let Some(method) = corpus.methods.get(method) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[method.name_start as usize..method.name_end as usize])
        .unwrap_or("")
}

pub fn rust_symbol_name(corpus: &RustMethodCorpus, symbol: usize) -> &str {
    let Some(symbol) = corpus.symbols.get(symbol) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[symbol.name_start as usize..symbol.name_end as usize])
        .unwrap_or("")
}

pub fn rust_symbol_path(corpus: &RustMethodCorpus, symbol: usize) -> Option<&std::path::Path> {
    let Some(symbol) = corpus.symbols.get(symbol) else {
        return None;
    };
    corpus
        .paths
        .get(symbol.path as usize)
        .map(std::path::PathBuf::as_path)
}

pub fn rust_method_path(corpus: &RustMethodCorpus, method: usize) -> Option<&std::path::Path> {
    let Some(method) = corpus.methods.get(method) else {
        return None;
    };
    corpus
        .paths
        .get(method.path as usize)
        .map(std::path::PathBuf::as_path)
}

pub fn rust_method_definition(
    buffer: &GapBuffer,
    cursor: usize,
    corpus: &RustMethodCorpus,
) -> Option<usize> {
    profiling::function_scope!();
    let length = buffer_len(buffer);
    let mut name_start = cursor.min(length);
    while name_start > 0 && rust_identifier_byte(buffer_byte(buffer, name_start - 1)) {
        name_start -= 1;
    }
    let mut name_end = cursor.min(length);
    while name_end < length && rust_identifier_byte(buffer_byte(buffer, name_end)) {
        name_end += 1;
    }
    if name_start == name_end || name_start == 0 || buffer_byte(buffer, name_start - 1) != b'.' {
        return None;
    }
    let receiver_end = name_start - 1;
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && rust_identifier_byte(buffer_byte(buffer, receiver_start - 1)) {
        receiver_start -= 1;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut owner = temp.vec(32);
    if !rust_explicit_type(buffer, receiver_start, receiver_end, &mut owner) {
        return None;
    }
    let first = corpus.methods.partition_point(|method| {
        &corpus.bytes[method.owner_start as usize..method.owner_end as usize] < owner.as_slice()
    });
    let last = corpus.methods.partition_point(|method| {
        &corpus.bytes[method.owner_start as usize..method.owner_end as usize] <= owner.as_slice()
    });
    corpus.methods[first..last]
        .iter()
        .position(|method| {
            let name = &corpus.bytes[method.name_start as usize..method.name_end as usize];
            name.len() == name_end - name_start
                && name
                    .iter()
                    .enumerate()
                    .all(|(offset, &byte)| byte == buffer_byte(buffer, name_start + offset))
        })
        .map(|method| first + method)
}

pub fn rust_method_complete(
    buffer: &GapBuffer,
    insertion_point: usize,
    corpus: &RustMethodCorpus,
    matches: &mut Vec<FuzzyMatch, impl Allocator>,
) -> Option<usize> {
    profiling::function_scope!();
    matches.clear();
    let Some((receiver_start, receiver_end, prefix_start)) =
        rust_member_expression(buffer, insertion_point)
    else {
        return None;
    };
    let temp = idno_std::mem().scratch().temp();
    let mut owner = temp.vec(32);
    if !rust_explicit_type(buffer, receiver_start, receiver_end, &mut owner) {
        return None;
    }
    let first = corpus.methods.partition_point(|method| {
        &corpus.bytes[method.owner_start as usize..method.owner_end as usize] < owner.as_slice()
    });
    let last = corpus.methods.partition_point(|method| {
        &corpus.bytes[method.owner_start as usize..method.owner_end as usize] <= owner.as_slice()
    });
    let mut labels = temp.vec(last - first);
    let mut method_indices = temp.vec(last - first);
    for (method_index, method) in corpus.methods[first..last].iter().enumerate() {
        let name = &corpus.bytes[method.name_start as usize..method.name_end as usize];
        labels.push(std::str::from_utf8(name).unwrap_or(""));
        method_indices.push(first + method_index);
    }
    let mut query = temp.vec(insertion_point - prefix_start);
    for position in prefix_start..insertion_point {
        query.push(buffer_byte(buffer, position));
    }
    let query = std::str::from_utf8(&query).unwrap_or("");
    fuzzy_rank(query, &labels, matches);
    for found in matches.iter_mut() {
        found.item = method_indices[found.item];
    }
    matches.truncate(32);
    Some(prefix_start)
}

fn rust_member_expression(
    buffer: &GapBuffer,
    insertion_point: usize,
) -> Option<(usize, usize, usize)> {
    let mut prefix_start = insertion_point.min(buffer_len(buffer));
    while prefix_start > 0 && rust_identifier_byte(buffer_byte(buffer, prefix_start - 1)) {
        prefix_start -= 1;
    }
    if prefix_start == 0 || buffer_byte(buffer, prefix_start - 1) != b'.' {
        return None;
    }
    let receiver_end = prefix_start - 1;
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && rust_identifier_byte(buffer_byte(buffer, receiver_start - 1)) {
        receiver_start -= 1;
    }
    (receiver_start < receiver_end).then_some((receiver_start, receiver_end, prefix_start))
}

fn rust_explicit_type(
    buffer: &GapBuffer,
    receiver_start: usize,
    receiver_end: usize,
    owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let mut search = receiver_start;
    while search > 0 {
        search -= 1;
        if !rust_range_equal(
            buffer,
            search,
            search + receiver_end - receiver_start,
            receiver_start,
            receiver_end,
        ) {
            continue;
        }
        let before_boundary = search == 0 || !rust_identifier_byte(buffer_byte(buffer, search - 1));
        let after_name = search + receiver_end - receiver_start;
        let after_boundary = after_name >= buffer_len(buffer)
            || !rust_identifier_byte(buffer_byte(buffer, after_name));
        if !before_boundary || !after_boundary {
            continue;
        }
        let mut position = after_name;
        rust_skip_buffer_whitespace(buffer, &mut position);
        if position >= buffer_len(buffer) || buffer_byte(buffer, position) != b':' {
            continue;
        }
        position += 1;
        rust_skip_buffer_whitespace(buffer, &mut position);
        while position < buffer_len(buffer) && matches!(buffer_byte(buffer, position), b'&' | b'\'')
        {
            position += 1;
        }
        if rust_buffer_word_equal(buffer, position, b"mut") {
            position += 3;
            rust_skip_buffer_whitespace(buffer, &mut position);
        }
        while position < buffer_len(buffer) {
            let start = position;
            while position < buffer_len(buffer)
                && rust_identifier_byte(buffer_byte(buffer, position))
            {
                position += 1;
            }
            if start == position {
                return false;
            }
            owner.clear();
            for byte_position in start..position {
                owner.push(buffer_byte(buffer, byte_position));
            }
            if position + 1 < buffer_len(buffer)
                && buffer_byte(buffer, position) == b':'
                && buffer_byte(buffer, position + 1) == b':'
            {
                position += 2;
                continue;
            }
            return true;
        }
    }
    false
}

fn rust_method_corpus_build(
    root: &std::path::Path,
    project_paths: &[std::path::PathBuf],
) -> RustMethodCorpus {
    profiling::function_scope!();
    let mut corpus = RustMethodCorpus {
        bytes: Vec::with_capacity(64 * 1024),
        methods: Vec::with_capacity(4096),
        symbols: Vec::with_capacity(4096),
        paths: Vec::with_capacity(512),
        standard_library_available: false,
    };
    let sysroot = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(root)
        .output();
    if let Ok(sysroot) = sysroot
        && sysroot.status.success()
        && let Ok(sysroot) = std::str::from_utf8(&sysroot.stdout)
    {
        let library =
            std::path::PathBuf::from(sysroot.trim_end()).join("lib/rustlib/src/rust/library");
        if library.is_dir() {
            corpus.standard_library_available = true;
            rust_index_directory(&library, &mut corpus);
        }
    }
    for path in project_paths {
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
            continue;
        }
        let source = match std::fs::read(path) {
            Ok(source) => source,
            Err(_) => continue,
        };
        rust_index_source(path, &source, &mut corpus);
    }
    corpus.methods.sort_unstable_by(|left, right| {
        let left_owner = &corpus.bytes[left.owner_start as usize..left.owner_end as usize];
        let right_owner = &corpus.bytes[right.owner_start as usize..right.owner_end as usize];
        let left_name = &corpus.bytes[left.name_start as usize..left.name_end as usize];
        let right_name = &corpus.bytes[right.name_start as usize..right.name_end as usize];
        (left_owner, left_name).cmp(&(right_owner, right_name))
    });
    corpus.methods.dedup_by(|right, left| {
        corpus.bytes[left.owner_start as usize..left.owner_end as usize]
            == corpus.bytes[right.owner_start as usize..right.owner_end as usize]
            && corpus.bytes[left.name_start as usize..left.name_end as usize]
                == corpus.bytes[right.name_start as usize..right.name_end as usize]
    });
    corpus
}

fn rust_index_directory(root: &std::path::Path, corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs")
                && let Ok(source) = std::fs::read(&path)
            {
                rust_index_source(&path, &source, corpus);
            }
        }
    }
}

fn rust_index_source(path: &std::path::Path, source: &[u8], corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    if corpus.paths.len() > u32::MAX as usize || source.len() > u32::MAX as usize {
        return;
    }
    let path_index = corpus.paths.len() as u32;
    corpus.paths.push(path.to_path_buf());
    rust_index_symbols(source, path_index, corpus);
    rust_index_methods(source, path_index, corpus);
}

fn rust_index_methods(source: &[u8], path: u32, corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    let mut position = 0;
    while position + 4 <= source.len() {
        if !rust_source_word(source, position, b"impl") {
            position += 1;
            continue;
        }
        let header_start = position + 4;
        let Some(open) = source[header_start..].iter().position(|&byte| byte == b'{') else {
            break;
        };
        let open = header_start + open;
        let Some((owner_start, owner_end)) = rust_impl_owner(&source[header_start..open]) else {
            position = open + 1;
            continue;
        };
        let owner_start = header_start + owner_start;
        let owner_end = header_start + owner_end;
        let mut depth = 1usize;
        let mut body = open + 1;
        while body < source.len() && depth > 0 {
            if source[body] == b'{' {
                depth += 1;
            } else if source[body] == b'}' {
                depth -= 1;
            } else if depth == 1 && rust_source_word(source, body, b"fn") {
                let mut name_start = body + 2;
                while name_start < source.len() && source[name_start].is_ascii_whitespace() {
                    name_start += 1;
                }
                let mut name_end = name_start;
                while name_end < source.len() && rust_identifier_byte(source[name_end]) {
                    name_end += 1;
                }
                if name_end > name_start {
                    rust_method_push(
                        corpus,
                        &source[owner_start..owner_end],
                        &source[name_start..name_end],
                        path,
                        name_start,
                        name_end,
                    );
                }
                body = name_end;
                continue;
            }
            body += 1;
        }
        position = body;
    }
}

fn rust_impl_owner(header: &[u8]) -> Option<(usize, usize)> {
    let mut start = 0;
    while start < header.len() && header[start].is_ascii_whitespace() {
        start += 1;
    }
    if header.get(start) == Some(&b'<') {
        let mut depth = 1usize;
        start += 1;
        while start < header.len() && depth > 0 {
            depth += usize::from(header[start] == b'<');
            depth = depth.saturating_sub(usize::from(header[start] == b'>'));
            start += 1;
        }
    }
    if let Some(for_position) = rust_find_word(header, start, b"for") {
        start = for_position + 3;
    }
    while start < header.len() && header[start].is_ascii_whitespace() {
        start += 1;
    }
    let mut owner_start = start;
    let mut owner_end = start;
    while start < header.len() {
        if rust_identifier_byte(header[start]) {
            let word_start = start;
            start += 1;
            while start < header.len() && rust_identifier_byte(header[start]) {
                start += 1;
            }
            owner_start = word_start;
            owner_end = start;
            if start >= header.len() || header[start] != b':' {
                break;
            }
            start += 2;
        } else {
            start += 1;
        }
    }
    (owner_end > owner_start).then_some((owner_start, owner_end))
}

fn rust_method_push(
    corpus: &mut RustMethodCorpus,
    owner: &[u8],
    name: &[u8],
    path: u32,
    position: usize,
    end: usize,
) {
    let owner_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(owner);
    let owner_end = corpus.bytes.len();
    let name_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(name);
    let name_end = corpus.bytes.len();
    if name_end > u32::MAX as usize {
        corpus.bytes.truncate(owner_start);
        return;
    }
    corpus.methods.push(RustMethod {
        owner_start: owner_start as u32,
        owner_end: owner_end as u32,
        name_start: name_start as u32,
        name_end: name_end as u32,
        path,
        position: position as u32,
        end: end as u32,
    });
}

fn rust_index_symbols(source: &[u8], path: u32, corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    let mut line_start = 0;
    while line_start < source.len() {
        let line_end = source[line_start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(source.len(), |offset| line_start + offset);
        if let Some((name_start, name_end)) = rust_symbol_line_name(&source[line_start..line_end]) {
            let name = &source[line_start + name_start..line_start + name_end];
            let stored_start = corpus.bytes.len();
            corpus.bytes.extend_from_slice(name);
            let stored_end = corpus.bytes.len();
            if stored_end <= u32::MAX as usize {
                corpus.symbols.push(RustSymbol {
                    name_start: stored_start as u32,
                    name_end: stored_end as u32,
                    path,
                    position: (line_start + name_start) as u32,
                    end: (line_start + name_end) as u32,
                });
            } else {
                corpus.bytes.truncate(stored_start);
                return;
            }
        }
        line_start = line_end.saturating_add(1);
    }
}

fn rust_symbol_line_name(line: &[u8]) -> Option<(usize, usize)> {
    let mut position = 0;
    rust_skip_source_whitespace(line, &mut position);
    loop {
        let start = position;
        while position < line.len() && rust_identifier_byte(line[position]) {
            position += 1;
        }
        let word = &line[start..position];
        if word == b"pub" {
            rust_skip_source_whitespace(line, &mut position);
            if line.get(position) == Some(&b'(') {
                let mut depth = 1usize;
                position += 1;
                while position < line.len() && depth > 0 {
                    depth += usize::from(line[position] == b'(');
                    depth = depth.saturating_sub(usize::from(line[position] == b')'));
                    position += 1;
                }
                rust_skip_source_whitespace(line, &mut position);
            }
            continue;
        }
        if matches!(word, b"async" | b"unsafe" | b"extern" | b"default") {
            rust_skip_source_whitespace(line, &mut position);
            continue;
        }
        if !matches!(
            word,
            b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"mod" | b"const" | b"static"
        ) {
            return None;
        }
        rust_skip_source_whitespace(line, &mut position);
        if word == b"static" && line.get(position..position + 4) == Some(b"mut ") {
            position += 4;
        }
        let name_start = position;
        while position < line.len() && rust_identifier_byte(line[position]) {
            position += 1;
        }
        return (position > name_start).then_some((name_start, position));
    }
}

fn rust_skip_source_whitespace(source: &[u8], position: &mut usize) {
    while *position < source.len() && source[*position].is_ascii_whitespace() {
        *position += 1;
    }
}

fn rust_find_word(source: &[u8], mut position: usize, word: &[u8]) -> Option<usize> {
    while position + word.len() <= source.len() {
        if rust_source_word(source, position, word) {
            return Some(position);
        }
        position += 1;
    }
    None
}

fn rust_source_word(source: &[u8], position: usize, word: &[u8]) -> bool {
    source.get(position..position + word.len()) == Some(word)
        && (position == 0 || !rust_identifier_byte(source[position - 1]))
        && (position + word.len() == source.len()
            || !rust_identifier_byte(source[position + word.len()]))
}

fn rust_range_equal(
    buffer: &GapBuffer,
    candidate_start: usize,
    candidate_end: usize,
    expected_start: usize,
    expected_end: usize,
) -> bool {
    let length = expected_end - expected_start;
    candidate_end - candidate_start == length
        && (0..length).all(|offset| {
            buffer_byte(buffer, candidate_start + offset)
                == buffer_byte(buffer, expected_start + offset)
        })
}

fn rust_buffer_word_equal(buffer: &GapBuffer, position: usize, word: &[u8]) -> bool {
    position + word.len() <= buffer_len(buffer)
        && word
            .iter()
            .enumerate()
            .all(|(offset, &byte)| buffer_byte(buffer, position + offset) == byte)
}

fn rust_skip_buffer_whitespace(buffer: &GapBuffer, position: &mut usize) {
    while *position < buffer_len(buffer) && buffer_byte(buffer, *position).is_ascii_whitespace() {
        *position += 1;
    }
}

#[inline]
fn rust_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::buffer_from_bytes;

    #[test]
    fn explicit_vec_type_completes_indexed_push_method() {
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: vec![std::path::PathBuf::from("test.rs")],
            standard_library_available: true,
        };
        rust_index_methods(
            b"impl<T> Vec<T> { pub fn push(&mut self, value: T) {} }",
            0,
            &mut corpus,
        );
        let buffer = buffer_from_bytes(b"let potato: Vec<usize>; potato.pu");
        let mut matches = Vec::new();
        let prefix = rust_method_complete(&buffer, buffer_len(&buffer), &corpus, &mut matches);
        assert_eq!(prefix, Some(31));
        assert_eq!(rust_method_name(&corpus, matches[0].item), "push");
    }

    #[test]
    fn indexes_top_level_symbols_and_method_locations() {
        let source = b"pub enum Option<T> { None, Some(T) }\nimpl<T> Option<T> { pub fn get_or_insert(&mut self, value: T) {} }";
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: true,
        };
        rust_index_source(std::path::Path::new("option.rs"), source, &mut corpus);
        assert_eq!(rust_symbol_name(&corpus, 0), "Option");
        let buffer = buffer_from_bytes(b"let value: Option<usize>; value.get_or_insert()");
        let method = rust_method_definition(&buffer, 34, &corpus).unwrap();
        assert_eq!(rust_method_name(&corpus, method), "get_or_insert");
        assert_eq!(corpus.methods[method].position, 63);
    }
}
