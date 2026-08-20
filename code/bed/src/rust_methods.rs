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
    pub detail_start: u32,
    pub detail_end: u32,
}

struct RustMethodSource<'a> {
    owner: &'a [u8],
    name: &'a [u8],
    signature: &'a [u8],
    documentation: &'a [u8],
}

#[derive(Clone, Copy)]
pub struct RustSymbol {
    pub owner_start: u32,
    pub owner_end: u32,
    pub name_start: u32,
    pub name_end: u32,
    pub path: u32,
    pub position: u32,
    pub end: u32,
    pub detail_start: u32,
    pub detail_end: u32,
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

pub fn rust_method_index_restart(
    index: &mut RustMethodIndex,
    root: &std::path::Path,
    project_paths: std::sync::Arc<Vec<std::path::PathBuf>>,
) {
    profiling::function_scope!();
    if let Some(task) = index.task.take() {
        task.cancel();
    }
    let root = root.to_path_buf();
    index.task = Some(
        idno_std::threads().spawn_owned(move || rust_method_corpus_build(&root, &project_paths)),
    );
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

pub fn rust_method_index_finish(index: &mut RustMethodIndex) -> bool {
    profiling::function_scope!();
    let Some(task) = index.task.take() else {
        return false;
    };
    index.corpus = task.join();
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

pub fn rust_method_detail(corpus: &RustMethodCorpus, method: usize) -> &str {
    let Some(method) = corpus.methods.get(method) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[method.detail_start as usize..method.detail_end as usize])
        .unwrap_or("")
}

pub fn rust_symbol_name(corpus: &RustMethodCorpus, symbol: usize) -> &str {
    let Some(symbol) = corpus.symbols.get(symbol) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[symbol.name_start as usize..symbol.name_end as usize])
        .unwrap_or("")
}

pub fn rust_symbol_owner(corpus: &RustMethodCorpus, symbol: usize) -> &str {
    let Some(symbol) = corpus.symbols.get(symbol) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[symbol.owner_start as usize..symbol.owner_end as usize])
        .unwrap_or("")
}

pub fn rust_symbol_detail(corpus: &RustMethodCorpus, symbol: usize) -> &str {
    let Some(symbol) = corpus.symbols.get(symbol) else {
        return "";
    };
    std::str::from_utf8(&corpus.bytes[symbol.detail_start as usize..symbol.detail_end as usize])
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

pub fn rust_namespace_root<'a>(
    corpus: &'a RustMethodCorpus,
    namespace: &[u8],
) -> Option<&'a std::path::Path> {
    profiling::function_scope!();
    corpus
        .symbols
        .iter()
        .enumerate()
        .find_map(|(symbol, definition)| {
            let Some(path) = rust_symbol_path(corpus, symbol) else {
                return None;
            };
            (definition.position == 0
                && rust_symbol_owner(corpus, symbol).is_empty()
                && rust_symbol_name(corpus, symbol).as_bytes() == namespace
                && path.file_name() == Some(std::ffi::OsStr::new("lib.rs")))
            .then(|| path.parent().and_then(std::path::Path::parent))
            .flatten()
        })
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
    let temp = idno_std::mem().scratch().temp();
    let mut owner = temp.vec(32);
    if !rust_receiver_type(buffer, receiver_end, corpus, &mut owner) {
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
    let mut prefix_start = insertion_point.min(buffer_len(buffer));
    while prefix_start > 0 && rust_identifier_byte(buffer_byte(buffer, prefix_start - 1)) {
        prefix_start -= 1;
    }
    if prefix_start == 0 || buffer_byte(buffer, prefix_start - 1) != b'.' {
        return None;
    }
    let receiver_end = prefix_start - 1;
    let temp = idno_std::mem().scratch().temp();
    let mut owner = temp.vec(32);
    if !rust_receiver_type(buffer, receiver_end, corpus, &mut owner) {
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

fn rust_receiver_type(
    buffer: &GapBuffer,
    receiver_end: usize,
    corpus: &RustMethodCorpus,
    owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let mut expression_end = receiver_end;
    while expression_end > 0 && buffer_byte(buffer, expression_end - 1).is_ascii_whitespace() {
        expression_end -= 1;
    }
    if expression_end == 0 {
        return false;
    }
    if buffer_byte(buffer, expression_end - 1) != b')' {
        let mut expression_start = expression_end;
        while expression_start > 0
            && rust_identifier_byte(buffer_byte(buffer, expression_start - 1))
        {
            expression_start -= 1;
        }
        if expression_start > 0 && buffer_byte(buffer, expression_start - 1) == b'.' {
            let mut receiver_start = expression_start - 1;
            while receiver_start > 0
                && rust_identifier_byte(buffer_byte(buffer, receiver_start - 1))
            {
                receiver_start -= 1;
            }
            let temp = idno_std::mem().scratch().temp();
            let mut receiver_type = temp.vec(32);
            if receiver_start < expression_start - 1
                && rust_explicit_type(
                    buffer,
                    receiver_start,
                    expression_start - 1,
                    &mut receiver_type,
                )
                && rust_struct_field_type(
                    buffer,
                    &receiver_type,
                    expression_start,
                    expression_end,
                    owner,
                )
            {
                return true;
            }
        }
        return expression_start < expression_end
            && rust_explicit_type(buffer, expression_start, expression_end, owner);
    }
    let mut position = expression_end - 1;
    let mut depth = 1usize;
    while position > 0 && depth > 0 {
        position -= 1;
        depth += usize::from(buffer_byte(buffer, position) == b')');
        depth = depth.saturating_sub(usize::from(buffer_byte(buffer, position) == b'('));
    }
    if depth != 0 {
        return false;
    }
    let mut name_end = position;
    while name_end > 0 && buffer_byte(buffer, name_end - 1).is_ascii_whitespace() {
        name_end -= 1;
    }
    let mut name_start = name_end;
    while name_start > 0 && rust_identifier_byte(buffer_byte(buffer, name_start - 1)) {
        name_start -= 1;
    }
    if name_start == name_end {
        return false;
    }
    let mut module_end = name_start;
    while module_end > 0 && buffer_byte(buffer, module_end - 1).is_ascii_whitespace() {
        module_end -= 1;
    }
    let mut module_start = module_end;
    if module_end >= 2
        && buffer_byte(buffer, module_end - 1) == b':'
        && buffer_byte(buffer, module_end - 2) == b':'
    {
        module_end -= 2;
        module_start = module_end;
        while module_start > 0 && rust_identifier_byte(buffer_byte(buffer, module_start - 1)) {
            module_start -= 1;
        }
    }
    for symbol in 0..corpus.symbols.len() {
        let definition = corpus.symbols[symbol];
        let name = &corpus.bytes[definition.name_start as usize..definition.name_end as usize];
        if name.len() != name_end - name_start
            || !name
                .iter()
                .enumerate()
                .all(|(offset, &byte)| byte == buffer_byte(buffer, name_start + offset))
        {
            continue;
        }
        if module_start < module_end {
            let Some(path) = rust_symbol_path(corpus, symbol) else {
                continue;
            };
            let module_matches = path
                .file_stem()
                .map(std::ffi::OsStr::as_encoded_bytes)
                .is_some_and(|candidate| {
                    candidate.len() == module_end - module_start
                        && candidate.iter().enumerate().all(|(offset, &byte)| {
                            byte == buffer_byte(buffer, module_start + offset)
                        })
                });
            if !module_matches {
                continue;
            }
        }
        if rust_callable_return_type(rust_symbol_detail(corpus, symbol).as_bytes(), owner) {
            return true;
        }
    }
    false
}

fn rust_struct_field_type(
    buffer: &GapBuffer,
    structure: &[u8],
    field_start: usize,
    field_end: usize,
    result: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let length = buffer_len(buffer);
    let mut position = 0usize;
    while position < length {
        if !rust_buffer_word_equal(buffer, position, b"struct") {
            position += 1;
            continue;
        }
        position += b"struct".len();
        rust_skip_buffer_whitespace(buffer, &mut position);
        let name_start = position;
        while position < length && rust_identifier_byte(buffer_byte(buffer, position)) {
            position += 1;
        }
        if position - name_start != structure.len()
            || !structure
                .iter()
                .enumerate()
                .all(|(offset, &byte)| buffer_byte(buffer, name_start + offset) == byte)
        {
            continue;
        }
        while position < length && buffer_byte(buffer, position) != b'{' {
            position += 1;
        }
        if position >= length {
            return false;
        }
        position += 1;
        let mut depth = 1usize;
        while position < length && depth > 0 {
            let byte = buffer_byte(buffer, position);
            if byte == b'{' {
                depth += 1;
                position += 1;
                continue;
            }
            if byte == b'}' {
                depth = depth.saturating_sub(1);
                position += 1;
                continue;
            }
            if depth != 1 || !rust_identifier_byte(byte) || byte.is_ascii_digit() {
                position += 1;
                continue;
            }
            let candidate_start = position;
            while position < length && rust_identifier_byte(buffer_byte(buffer, position)) {
                position += 1;
            }
            let candidate_end = position;
            rust_skip_buffer_whitespace(buffer, &mut position);
            if candidate_end - candidate_start != field_end - field_start
                || !(0..field_end - field_start).all(|offset| {
                    buffer_byte(buffer, candidate_start + offset)
                        == buffer_byte(buffer, field_start + offset)
                })
                || position >= length
                || buffer_byte(buffer, position) != b':'
                || (position + 1 < length && buffer_byte(buffer, position + 1) == b':')
            {
                continue;
            }
            position += 1;
            rust_skip_buffer_whitespace(buffer, &mut position);
            while position < length && matches!(buffer_byte(buffer, position), b'&' | b'\'') {
                position += 1;
            }
            if rust_buffer_word_equal(buffer, position, b"mut") {
                position += 3;
                rust_skip_buffer_whitespace(buffer, &mut position);
            }
            result.clear();
            loop {
                let type_start = position;
                while position < length && rust_identifier_byte(buffer_byte(buffer, position)) {
                    position += 1;
                }
                if type_start == position {
                    return false;
                }
                result.clear();
                for byte_position in type_start..position {
                    result.push(buffer_byte(buffer, byte_position));
                }
                if position + 1 >= length
                    || buffer_byte(buffer, position) != b':'
                    || buffer_byte(buffer, position + 1) != b':'
                {
                    return true;
                }
                position += 2;
            }
        }
        return false;
    }
    false
}

fn rust_callable_return_type(detail: &[u8], result: &mut Vec<u8, impl Allocator>) -> bool {
    result.clear();
    let mut position = 0usize;
    while position + 1 < detail.len() {
        if detail[position] == b'-' && detail[position + 1] == b'>' {
            position += 2;
            break;
        }
        position += 1;
    }
    while position < detail.len() && !rust_identifier_byte(detail[position]) {
        position += 1;
    }
    while position < detail.len() && rust_identifier_byte(detail[position]) {
        result.push(detail[position]);
        position += 1;
    }
    !result.is_empty()
}

pub fn rust_free_function_complete(
    buffer: &GapBuffer,
    insertion_point: usize,
    corpus: &RustMethodCorpus,
    matches: &mut Vec<FuzzyMatch, impl Allocator>,
) -> Option<(usize, usize, usize)> {
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
    let mut labels = temp.vec(64);
    let mut symbols = temp.vec(64);
    for symbol in 0..corpus.symbols.len() {
        let detail = rust_symbol_detail(corpus, symbol).as_bytes();
        if !rust_first_parameter_accepts(detail, &owner) {
            continue;
        }
        labels.push(rust_symbol_name(corpus, symbol));
        symbols.push(symbol);
    }
    let mut query = temp.vec(insertion_point - prefix_start);
    for position in prefix_start..insertion_point {
        query.push(buffer_byte(buffer, position));
    }
    let query = std::str::from_utf8(&query).unwrap_or("");
    fuzzy_rank(query, &labels, matches);
    for found in matches.iter_mut() {
        found.item = symbols[found.item];
    }
    matches.truncate(32);
    Some((prefix_start, receiver_start, receiver_end))
}

fn rust_first_parameter_accepts(detail: &[u8], owner: &[u8]) -> bool {
    let Some(open) = detail.iter().position(|&byte| byte == b'(') else {
        return false;
    };
    let end = detail[open + 1..]
        .iter()
        .position(|&byte| matches!(byte, b',' | b')'))
        .map_or(detail.len(), |offset| open + 1 + offset);
    let Some(colon) = detail[open + 1..end]
        .iter()
        .position(|&byte| byte == b':')
        .map(|offset| open + 1 + offset)
    else {
        return false;
    };
    let parameter_type = &detail[colon + 1..end];
    parameter_type
        .windows(owner.len())
        .enumerate()
        .any(|(position, candidate)| {
            candidate == owner
                && (position == 0 || !rust_identifier_byte(parameter_type[position - 1]))
                && (position + owner.len() == parameter_type.len()
                    || !rust_identifier_byte(parameter_type[position + owner.len()]))
        })
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

pub fn rust_explicit_type(
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
        if position >= buffer_len(buffer)
            || !matches!(buffer_byte(buffer, position), b':' | b'=')
            || (buffer_byte(buffer, position) == b':'
                && position + 1 < buffer_len(buffer)
                && buffer_byte(buffer, position + 1) == b':')
        {
            continue;
        }
        if buffer_byte(buffer, position) == b'=' && !rust_identifier_is_let_binding(buffer, search)
        {
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

fn rust_identifier_is_let_binding(buffer: &GapBuffer, identifier_start: usize) -> bool {
    let mut position = identifier_start;
    while position > 0 && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    let mut word_start = position;
    while word_start > 0 && rust_identifier_byte(buffer_byte(buffer, word_start - 1)) {
        word_start -= 1;
    }
    if word_start == position {
        return false;
    }
    if position - word_start == 3 && rust_buffer_word_equal(buffer, word_start, b"mut") {
        position = word_start;
        while position > 0 && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
            position -= 1;
        }
        word_start = position;
        while word_start > 0 && rust_identifier_byte(buffer_byte(buffer, word_start - 1)) {
            word_start -= 1;
        }
    }
    position - word_start == 3 && rust_buffer_word_equal(buffer, word_start, b"let")
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
    rust_index_cargo_dependencies(root, &mut corpus);
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
    let temp = idno_std::mem().scratch().temp();
    let mut directories = temp.vec(512);
    directories.push(root.to_path_buf());
    while let Some(directory) = directories.pop() {
        rust_index_crate_root(&directory, corpus);
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

fn rust_index_cargo_dependencies(root: &std::path::Path, corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    let metadata = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output();
    let metadata = match metadata {
        Ok(metadata) if metadata.status.success() => metadata.stdout,
        _ => return,
    };
    let marker = b"\"manifest_path\":\"";
    let mut position = 0usize;
    while position + marker.len() <= metadata.len() {
        let Some(found) = metadata[position..]
            .windows(marker.len())
            .position(|candidate| candidate == marker)
        else {
            break;
        };
        let start = position + found + marker.len();
        let Some(length) = metadata[start..].iter().position(|&byte| byte == b'"') else {
            break;
        };
        let path = match std::str::from_utf8(&metadata[start..start + length]) {
            Ok(path) => std::path::Path::new(path),
            Err(_) => {
                position = start + length + 1;
                continue;
            }
        };
        if let Some(package) = path.parent() {
            rust_index_crate_root(package, corpus);
            let source = package.join("src");
            if source.is_dir() {
                rust_index_directory(&source, corpus);
            }
        }
        position = start + length + 1;
    }
}

fn rust_index_crate_root(directory: &std::path::Path, corpus: &mut RustMethodCorpus) {
    let manifest = match std::fs::read(directory.join("Cargo.toml")) {
        Ok(manifest) => manifest,
        Err(_) => return,
    };
    let Some(name) = rust_manifest_package_name(&manifest) else {
        return;
    };
    let path = directory.join("src/lib.rs");
    let source = match std::fs::read(&path) {
        Ok(source) => source,
        Err(_) => return,
    };
    rust_index_source(&path, &source, corpus);
    let Some(path_index) = corpus.paths.iter().position(|candidate| candidate == &path) else {
        return;
    };
    let temp = idno_std::mem().scratch().temp();
    let mut rust_name = temp.vec(name.len());
    rust_name.extend(
        name.iter()
            .map(|&byte| if byte == b'-' { b'_' } else { byte }),
    );
    rust_symbol_push(
        corpus,
        &[],
        &rust_name,
        &[],
        path_index as u32,
        0,
        source.len(),
    );
}

fn rust_manifest_package_name(manifest: &[u8]) -> Option<&[u8]> {
    let mut in_package = false;
    for line in manifest.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let mut position = 0usize;
        while position < line.len() && line[position].is_ascii_whitespace() {
            position += 1;
        }
        if line.get(position) == Some(&b'[') {
            in_package = line.get(position..position + 9) == Some(b"[package]");
            continue;
        }
        if !in_package || line.get(position..position + 4) != Some(b"name") {
            continue;
        }
        position += 4;
        rust_skip_source_whitespace(line, &mut position);
        if line.get(position) != Some(&b'=') {
            continue;
        }
        position += 1;
        rust_skip_source_whitespace(line, &mut position);
        if line.get(position) != Some(&b'"') {
            continue;
        }
        position += 1;
        let start = position;
        while position < line.len() && line[position] != b'"' {
            position += 1;
        }
        return (position > start).then_some(&line[start..position]);
    }
    None
}

fn rust_index_source(path: &std::path::Path, source: &[u8], corpus: &mut RustMethodCorpus) {
    profiling::function_scope!();
    if corpus.paths.len() > u32::MAX as usize || source.len() > u32::MAX as usize {
        return;
    }
    if corpus.paths.iter().any(|candidate| candidate == path) {
        return;
    }
    let path_index = corpus.paths.len() as u32;
    corpus.paths.push(path.to_path_buf());
    rust_index_symbols(source, path_index, corpus);
    rust_index_methods(source, path_index, corpus);
    rust_index_primitive_macro_methods(path, source, path_index, corpus);
}

fn rust_index_primitive_macro_methods(
    path: &std::path::Path,
    source: &[u8],
    path_index: u32,
    corpus: &mut RustMethodCorpus,
) {
    profiling::function_scope!();
    let owners: &[&[u8]] = match path.file_name().and_then(std::ffi::OsStr::to_str) {
        Some("uint_macros.rs") => &[b"u8", b"u16", b"u32", b"u64", b"u128", b"usize"],
        Some("int_macros.rs") => &[b"i8", b"i16", b"i32", b"i64", b"i128", b"isize"],
        _ => return,
    };
    let mut line_start = 0usize;
    while line_start < source.len() {
        let line_end = source[line_start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(source.len(), |offset| line_start + offset);
        let line = &source[line_start..line_end];
        let Some((name_start, name_end)) = rust_symbol_line_name(line) else {
            line_start = line_end.saturating_add(1);
            continue;
        };
        if rust_find_word(&line[..name_start], 0, b"fn").is_none() {
            line_start = line_end.saturating_add(1);
            continue;
        }
        for &owner in owners {
            rust_method_push(
                corpus,
                RustMethodSource {
                    owner,
                    name: &line[name_start..name_end],
                    signature: line,
                    documentation: &[],
                },
                path_index,
                line_start + name_start,
                line_start + name_end,
            );
        }
        line_start = line_end.saturating_add(1);
    }
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
                    let detail_start = rust_method_detail_start(source, body);
                    let mut detail_end = name_end;
                    while detail_end < source.len()
                        && !matches!(source[detail_end], b'{' | b';' | b'\n')
                    {
                        detail_end += 1;
                    }
                    rust_method_push(
                        corpus,
                        RustMethodSource {
                            owner: &source[owner_start..owner_end],
                            name: &source[name_start..name_end],
                            signature: &source[body..detail_end],
                            documentation: &source[detail_start..body],
                        },
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

fn rust_method_detail_start(source: &[u8], function_start: usize) -> usize {
    let mut start = function_start;
    while start > 0 && source[start - 1] != b'\n' {
        start -= 1;
    }
    loop {
        if start == 0 {
            return start;
        }
        let previous_end = start - 1;
        let mut previous_start = previous_end;
        while previous_start > 0 && source[previous_start - 1] != b'\n' {
            previous_start -= 1;
        }
        let mut text = previous_start;
        while text < previous_end && source[text].is_ascii_whitespace() {
            text += 1;
        }
        if source.get(text..text + 3) != Some(b"///")
            && source.get(text..text + 3) != Some(b"//!")
            && source.get(text..text + 6) != Some(b"#[doc ")
        {
            return start;
        }
        start = previous_start;
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
    source: RustMethodSource<'_>,
    path: u32,
    position: usize,
    end: usize,
) {
    let owner_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(source.owner);
    let owner_end = corpus.bytes.len();
    let name_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(source.name);
    let name_end = corpus.bytes.len();
    let detail_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(source.signature);
    if !source.documentation.is_empty() {
        corpus.bytes.push(b'\n');
        corpus.bytes.extend_from_slice(source.documentation);
    }
    let detail_end = corpus.bytes.len();
    if detail_end > u32::MAX as usize {
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
        detail_start: detail_start as u32,
        detail_end: detail_end as u32,
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
        let line = &source[line_start..line_end];
        let symbol = rust_symbol_line_name(line).or_else(|| rust_symbol_line_generated_type(line));
        if let Some((name_start, name_end)) = symbol {
            let name = &source[line_start + name_start..line_start + name_end];
            let declaration_start = line_start
                + line
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .unwrap_or(name_start);
            let declaration_end = rust_declaration_detail_end(source, declaration_start);
            let documentation_start = rust_method_detail_start(source, declaration_start);
            let temp = idno_std::mem().scratch().temp();
            let mut detail = temp.vec(declaration_end - documentation_start + 1);
            detail.extend_from_slice(&source[declaration_start..declaration_end]);
            if documentation_start < declaration_start {
                detail.push(b'\n');
                detail.extend_from_slice(&source[documentation_start..declaration_start]);
            }
            rust_symbol_push(
                corpus,
                &[],
                name,
                &detail,
                path,
                line_start + name_start,
                line_start + name_end,
            );
            if rust_line_item_is_enum(&source[line_start..line_end], name_start) {
                rust_index_enum_variants(source, line_start + name_end, name, path, corpus);
            }
        }
        line_start = line_end.saturating_add(1);
    }
}

fn rust_symbol_line_generated_type(line: &[u8]) -> Option<(usize, usize)> {
    let mut position = 0usize;
    rust_skip_source_whitespace(line, &mut position);
    let first_start = position;
    while position < line.len() && rust_identifier_byte(line[position]) {
        position += 1;
    }
    if position == first_start || !line[first_start].is_ascii_lowercase() {
        return None;
    }
    rust_skip_source_whitespace(line, &mut position);
    let name_start = position;
    while position < line.len() && rust_identifier_byte(line[position]) {
        position += 1;
    }
    if position == name_start || !line[name_start].is_ascii_uppercase() {
        return None;
    }
    let name_end = position;
    rust_skip_source_whitespace(line, &mut position);
    (position == line.len() || line.get(position) == Some(&b',')).then_some((name_start, name_end))
}

fn rust_symbol_push(
    corpus: &mut RustMethodCorpus,
    owner: &[u8],
    name: &[u8],
    detail: &[u8],
    path: u32,
    position: usize,
    end: usize,
) {
    let stored_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(owner);
    let owner_end = corpus.bytes.len();
    let name_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(name);
    let name_end = corpus.bytes.len();
    let detail_start = corpus.bytes.len();
    corpus.bytes.extend_from_slice(detail);
    let detail_end = corpus.bytes.len();
    if detail_end > u32::MAX as usize || position > u32::MAX as usize || end > u32::MAX as usize {
        corpus.bytes.truncate(stored_start);
        return;
    }
    corpus.symbols.push(RustSymbol {
        owner_start: stored_start as u32,
        owner_end: owner_end as u32,
        name_start: name_start as u32,
        name_end: name_end as u32,
        path,
        position: position as u32,
        end: end as u32,
        detail_start: detail_start as u32,
        detail_end: detail_end as u32,
    });
}

fn rust_line_item_is_enum(line: &[u8], name_start: usize) -> bool {
    let mut position = 0;
    while position < name_start {
        if rust_source_word(line, position, b"enum") {
            return true;
        }
        position += 1;
    }
    false
}

fn rust_index_enum_variants(
    source: &[u8],
    after_name: usize,
    owner: &[u8],
    path: u32,
    corpus: &mut RustMethodCorpus,
) {
    profiling::function_scope!();
    let Some(relative_open) = source[after_name..].iter().position(|&byte| byte == b'{') else {
        return;
    };
    let mut position = after_name + relative_open + 1;
    let mut curly_depth = 1usize;
    let mut round_depth = 0usize;
    let mut square_depth = 0usize;
    let mut candidate_allowed = true;
    while position < source.len() && curly_depth > 0 {
        let byte = source[position];
        let next = source.get(position + 1).copied();
        if byte == b'/' && next == Some(b'/') {
            position += 2;
            while position < source.len() && source[position] != b'\n' {
                position += 1;
            }
            continue;
        }
        if byte == b'/' && next == Some(b'*') {
            position += 2;
            while position + 1 < source.len() && source.get(position..position + 2) != Some(b"*/") {
                position += 1;
            }
            position = (position + 2).min(source.len());
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            let delimiter = byte;
            position += 1;
            while position < source.len() {
                if source[position] == b'\\' {
                    position = (position + 2).min(source.len());
                } else if source[position] == delimiter {
                    position += 1;
                    break;
                } else {
                    position += 1;
                }
            }
            continue;
        }
        match byte {
            b'{' => curly_depth += 1,
            b'}' => curly_depth = curly_depth.saturating_sub(1),
            b'(' => round_depth += 1,
            b')' => round_depth = round_depth.saturating_sub(1),
            b'[' => square_depth += 1,
            b']' => square_depth = square_depth.saturating_sub(1),
            b',' if curly_depth == 1 && round_depth == 0 && square_depth == 0 => {
                candidate_allowed = true;
            }
            _ => {}
        }
        if curly_depth == 1
            && round_depth == 0
            && square_depth == 0
            && candidate_allowed
            && byte.is_ascii_uppercase()
        {
            let name_start = position;
            position += 1;
            while position < source.len() && rust_identifier_byte(source[position]) {
                position += 1;
            }
            let name_end = position;
            let declaration_start = name_start;
            let documentation_start = rust_method_detail_start(source, declaration_start);
            let mut declaration_end = name_end;
            let mut declaration_round_depth = 0usize;
            let mut declaration_square_depth = 0usize;
            let mut declaration_curly_depth = 0usize;
            while declaration_end < source.len() {
                match source[declaration_end] {
                    b'(' => declaration_round_depth += 1,
                    b')' => declaration_round_depth = declaration_round_depth.saturating_sub(1),
                    b'[' => declaration_square_depth += 1,
                    b']' => declaration_square_depth = declaration_square_depth.saturating_sub(1),
                    b'{' => declaration_curly_depth += 1,
                    b'}' if declaration_curly_depth > 0 => declaration_curly_depth -= 1,
                    b',' if declaration_round_depth == 0
                        && declaration_square_depth == 0
                        && declaration_curly_depth == 0 =>
                    {
                        break;
                    }
                    b'\n'
                        if declaration_round_depth == 0
                            && declaration_square_depth == 0
                            && declaration_curly_depth == 0 =>
                    {
                        break;
                    }
                    _ => {}
                }
                declaration_end += 1;
            }
            while declaration_end > declaration_start
                && source[declaration_end - 1].is_ascii_whitespace()
            {
                declaration_end -= 1;
            }
            let temp = idno_std::mem().scratch().temp();
            let mut detail = temp.vec(declaration_end - documentation_start + 1);
            detail.extend_from_slice(&source[declaration_start..declaration_end]);
            if documentation_start < declaration_start {
                detail.push(b'\n');
                detail.extend_from_slice(&source[documentation_start..declaration_start]);
            }
            rust_symbol_push(
                corpus,
                owner,
                &source[name_start..name_end],
                &detail,
                path,
                name_start,
                name_end,
            );
            candidate_allowed = false;
            continue;
        }
        position += 1;
    }
}

fn rust_declaration_detail_end(source: &[u8], start: usize) -> usize {
    let mut position = start;
    let mut round_depth = 0usize;
    let mut square_depth = 0usize;
    while position < source.len() {
        match source[position] {
            b'(' => round_depth += 1,
            b')' => round_depth = round_depth.saturating_sub(1),
            b'[' => square_depth += 1,
            b']' => square_depth = square_depth.saturating_sub(1),
            b'{' | b';' if round_depth == 0 && square_depth == 0 => break,
            _ => {}
        }
        position += 1;
    }
    while position > start && source[position - 1].is_ascii_whitespace() {
        position -= 1;
    }
    position
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
        if word == b"const" {
            let mut next = position;
            rust_skip_source_whitespace(line, &mut next);
            let next_start = next;
            while next < line.len() && rust_identifier_byte(line[next]) {
                next += 1;
            }
            if matches!(&line[next_start..next], b"unsafe" | b"trait" | b"fn") {
                rust_skip_source_whitespace(line, &mut position);
                continue;
            }
        }
        if matches!(word, b"async" | b"unsafe" | b"extern" | b"default") {
            rust_skip_source_whitespace(line, &mut position);
            continue;
        }
        if word == b"macro_rules" {
            if line.get(position) == Some(&b'!') {
                position += 1;
            }
            rust_skip_source_whitespace(line, &mut position);
            let name_start = position;
            while position < line.len() && rust_identifier_byte(line[position]) {
                position += 1;
            }
            return (position > name_start).then_some((name_start, position));
        }
        if !matches!(
            word,
            b"fn"
                | b"struct"
                | b"enum"
                | b"trait"
                | b"type"
                | b"mod"
                | b"const"
                | b"static"
                | b"macro"
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
    fn finishing_an_index_publishes_its_background_corpus() {
        let mut index = rust_method_index_empty();
        index.task = Some(idno_std::threads().spawn_owned(|| RustMethodCorpus {
            bytes: b"Potato".to_vec(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: true,
        }));
        assert!(rust_method_index_finish(&mut index));
        assert!(index.task.is_none());
        assert_eq!(index.corpus.bytes, b"Potato");
        assert!(index.corpus.standard_library_available);
    }

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
        assert_eq!(rust_symbol_name(&corpus, 1), "None");
        assert_eq!(rust_symbol_owner(&corpus, 1), "Option");
        assert_eq!(rust_symbol_name(&corpus, 2), "Some");
        assert_eq!(rust_symbol_owner(&corpus, 2), "Option");
        let buffer = buffer_from_bytes(b"let value: Option<usize>; value.get_or_insert()");
        let method = rust_method_definition(&buffer, 34, &corpus).unwrap();
        assert_eq!(rust_method_name(&corpus, method), "get_or_insert");
        assert_eq!(corpus.methods[method].position, 64);
    }

    #[test]
    fn enum_variants_retain_their_declaration_and_documentation() {
        let source = b"pub enum Mode {\n    /// `r\"hello\"`\n    RawStr,\n}";
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: false,
        };
        rust_index_source(std::path::Path::new("mode.rs"), source, &mut corpus);
        let symbol = corpus
            .symbols
            .iter()
            .enumerate()
            .find_map(|(symbol, _)| {
                (rust_symbol_name(&corpus, symbol) == "RawStr").then_some(symbol)
            })
            .unwrap();
        let detail = rust_symbol_detail(&corpus, symbol);
        assert!(detail.starts_with("RawStr"));
        assert!(detail.contains("/// `r\"hello\"`"));
    }

    #[test]
    fn top_level_functions_retain_signatures_docs_and_complete_as_free_methods() {
        let source = b"/// Runs one editor step.\npub fn editor_run(\n    editor: &mut Editor,\n    terminal: &mut Terminal,\n) {}";
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: false,
        };
        rust_index_source(std::path::Path::new("editor.rs"), source, &mut corpus);
        let symbol = corpus
            .symbols
            .iter()
            .enumerate()
            .find_map(|(symbol, _)| {
                (rust_symbol_name(&corpus, symbol) == "editor_run").then_some(symbol)
            })
            .unwrap();
        let detail = rust_symbol_detail(&corpus, symbol);
        assert!(detail.contains("pub fn editor_run"));
        assert!(detail.contains("Runs one editor step"));

        let buffer = buffer_from_bytes(b"fn caller(editor: &mut Editor) { editor.run");
        let mut matches = Vec::new();
        let expression =
            rust_free_function_complete(&buffer, buffer_len(&buffer), &corpus, &mut matches);
        assert_eq!(expression, Some((40, 33, 39)));
        assert_eq!(matches[0].item, symbol);
    }

    #[test]
    fn method_definition_propagates_a_qualified_call_return_type() {
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: true,
        };
        rust_index_source(
            std::path::Path::new("library/std/src/env.rs"),
            b"pub fn temp_dir() -> PathBuf { loop {} }",
            &mut corpus,
        );
        rust_index_source(
            std::path::Path::new("library/std/src/path.rs"),
            b"impl PathBuf { pub fn join<P>(&self, path: P) -> PathBuf {} }",
            &mut corpus,
        );
        let source = b"let directory = std::env::temp_dir().join(format!(\"x\"));";
        let buffer = buffer_from_bytes(source);
        let cursor = source.windows(4).position(|word| word == b"join").unwrap();
        let method = rust_method_definition(&buffer, cursor, &corpus).unwrap();
        assert_eq!(rust_method_name(&corpus, method), "join");
    }

    #[test]
    fn indexes_declarative_macro_names() {
        let source = b"#[macro_export]\nmacro_rules! println { () => {} }\npub macro newer() {}\npub const unsafe trait Allocator {}";
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: false,
        };
        rust_index_source(std::path::Path::new("macros.rs"), source, &mut corpus);
        assert!(
            corpus
                .symbols
                .iter()
                .enumerate()
                .any(|(symbol, _)| rust_symbol_name(&corpus, symbol) == "println")
        );
        assert!(
            corpus
                .symbols
                .iter()
                .enumerate()
                .any(|(symbol, _)| rust_symbol_name(&corpus, symbol) == "Allocator")
        );
        assert!(
            corpus
                .symbols
                .iter()
                .enumerate()
                .any(|(symbol, _)| rust_symbol_name(&corpus, symbol) == "newer")
        );
    }

    #[test]
    #[ignore = "scans Cargo metadata and the installed Rust source tree"]
    fn real_corpus_contains_qualified_standard_aliases_and_workspace_macros() {
        let corpus =
            rust_method_corpus_build(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), &[]);
        let result = corpus.symbols.iter().enumerate().any(|(symbol, _)| {
            rust_symbol_name(&corpus, symbol) == "Result"
                && rust_symbol_path(&corpus, symbol).is_some_and(|path| {
                    path.components()
                        .any(|component| component.as_os_str() == "io")
                })
        });
        let bitfield = corpus.symbols.iter().enumerate().any(|(symbol, _)| {
            rust_symbol_name(&corpus, symbol) == "bitfield"
                && rust_symbol_path(&corpus, symbol).is_some_and(|path| {
                    path.components()
                        .any(|component| component.as_os_str() == "bitfield")
                })
        });
        let option = corpus
            .symbols
            .iter()
            .enumerate()
            .any(|(symbol, _)| rust_symbol_name(&corpus, symbol) == "Option");
        let vec = corpus
            .symbols
            .iter()
            .enumerate()
            .any(|(symbol, _)| rust_symbol_name(&corpus, symbol) == "Vec");
        let is_some = corpus.methods.iter().enumerate().any(|(method, _)| {
            rust_method_name(&corpus, method) == "is_some"
                && corpus.bytes[corpus.methods[method].owner_start as usize
                    ..corpus.methods[method].owner_end as usize]
                    == *b"Option"
        });
        assert!(result, "std::io::Result was not indexed");
        assert!(bitfield, "bitfield::bitfield! was not indexed");
        assert!(option, "prelude Option was not indexed");
        assert!(vec, "prelude Vec was not indexed");
        assert!(is_some, "Option::is_some was not indexed");

        let editor_source = include_bytes!("editor.rs");
        let editor_buffer = buffer_from_bytes(editor_source);
        let usage = editor_source
            .windows(b"editor.picker.is_some".len())
            .position(|window| window == b"editor.picker.is_some")
            .unwrap()
            + b"editor.picker.".len();
        let picker_end = usage - 1;
        let picker_start = picker_end - b"picker".len();
        let editor_start = picker_start - b"editor.".len();
        let editor_end = editor_start + b"editor".len();
        let mut owner = Vec::new();
        assert!(rust_explicit_type(
            &editor_buffer,
            editor_start,
            editor_end,
            &mut owner
        ));
        assert_eq!(owner, b"Editor");
        let receiver_type = owner.clone();
        assert!(rust_struct_field_type(
            &editor_buffer,
            &receiver_type,
            picker_start,
            picker_end,
            &mut owner,
        ));
        assert_eq!(owner, b"Option");
        let method = rust_method_definition(&editor_buffer, usage, &corpus).unwrap();
        assert_eq!(rust_method_name(&corpus, method), "is_some");
    }

    #[test]
    fn indexes_integer_methods_generated_from_standard_library_macro_tables() {
        let source = b"pub const fn next_multiple_of(self, rhs: Self) -> Self { self }";
        let mut corpus = RustMethodCorpus {
            bytes: Vec::new(),
            methods: Vec::new(),
            symbols: Vec::new(),
            paths: Vec::new(),
            standard_library_available: true,
        };
        rust_index_source(
            std::path::Path::new("core/src/num/uint_macros.rs"),
            source,
            &mut corpus,
        );
        corpus.methods.sort_unstable_by(|left, right| {
            let left_owner = &corpus.bytes[left.owner_start as usize..left.owner_end as usize];
            let right_owner = &corpus.bytes[right.owner_start as usize..right.owner_end as usize];
            let left_name = &corpus.bytes[left.name_start as usize..left.name_end as usize];
            let right_name = &corpus.bytes[right.name_start as usize..right.name_end as usize];
            (left_owner, left_name).cmp(&(right_owner, right_name))
        });
        let buffer = buffer_from_bytes(b"let size: usize = 7; size.next_multiple_of(4)");
        let method = rust_method_definition(&buffer, 32, &corpus).unwrap();
        assert_eq!(rust_method_name(&corpus, method), "next_multiple_of");
    }
}
