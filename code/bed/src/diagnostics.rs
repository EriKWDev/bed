#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

pub struct Diagnostic {
    pub path: std::path::PathBuf,
    pub display: String,
    pub message_start: u32,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub severity: DiagnosticSeverity,
}

pub struct DiagnosticsResult {
    pub diagnostics: Vec<Diagnostic>,
    pub available: bool,
}

pub struct Diagnostics {
    pub published: Vec<Diagnostic>,
    pub task: Option<idno_std::micropool::OwnedTask<DiagnosticsResult>>,
    pub root: Option<std::path::PathBuf>,
    pub available: bool,
}

pub fn diagnostics_start(root: &std::path::Path) -> Diagnostics {
    profiling::function_scope!();
    let root = diagnostics_cargo_root(root);
    let task = if cfg!(test) {
        None
    } else {
        root.as_ref().map(|root| {
            let root = root.clone();
            idno_std::threads().spawn_owned(move || diagnostics_collect(&root))
        })
    };
    Diagnostics {
        published: Vec::new(),
        task,
        root,
        available: false,
    }
}

pub fn diagnostics_restart(diagnostics: &mut Diagnostics) {
    profiling::function_scope!();
    if diagnostics.task.is_some() {
        return;
    }
    let Some(root) = diagnostics.root.clone() else {
        return;
    };
    diagnostics.task = Some(idno_std::threads().spawn_owned(move || diagnostics_collect(&root)));
}

pub fn diagnostics_poll(diagnostics: &mut Diagnostics) -> bool {
    profiling::function_scope!();
    let complete = diagnostics
        .task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let Some(task) = diagnostics.task.take() else {
        return false;
    };
    match task.try_join() {
        Ok(result) => {
            diagnostics.published = result.diagnostics;
            diagnostics.available = result.available;
            true
        }
        Err(task) => {
            diagnostics.task = Some(task);
            false
        }
    }
}

pub fn diagnostics_pending(diagnostics: &Diagnostics) -> bool {
    diagnostics.task.is_some()
}

fn diagnostics_cargo_root(root: &std::path::Path) -> Option<std::path::PathBuf> {
    root.ancestors()
        .find(|directory| directory.join("Cargo.toml").is_file())
        .map(std::path::Path::to_path_buf)
}

fn diagnostics_collect(root: &std::path::Path) -> DiagnosticsResult {
    profiling::function_scope!();
    let output = std::process::Command::new("cargo")
        .args(["check", "--workspace", "--message-format=json"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return DiagnosticsResult {
            diagnostics: Vec::new(),
            available: false,
        };
    };
    let mut diagnostics = Vec::with_capacity(128);
    for line in output.stdout.split(|&byte| byte == b'\n') {
        diagnostics_parse_line(root, line, &mut diagnostics);
    }
    diagnostics.sort_unstable_by(|left, right| {
        (left.severity, &left.path, left.line, left.column).cmp(&(
            right.severity,
            &right.path,
            right.line,
            right.column,
        ))
    });
    DiagnosticsResult {
        diagnostics,
        available: true,
    }
}

fn diagnostics_parse_line(
    root: &std::path::Path,
    source: &[u8],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !source
        .windows(b"\"reason\":\"compiler-message\"".len())
        .any(|window| window == b"\"reason\":\"compiler-message\"")
    {
        return;
    }
    let Some(primary) = bytes_find(source, b"\"is_primary\":true") else {
        return;
    };
    let Some(span) = json_object_containing(source, primary) else {
        return;
    };
    let Some(file_key) = bytes_rfind(span, b"\"file_name\":") else {
        return;
    };
    let Some((file_name, _)) = json_string(span, file_key + b"\"file_name\":".len()) else {
        return;
    };
    let line = json_u32_after(span, b"\"line_start\":").unwrap_or(1);
    let column = json_u32_after(span, b"\"column_start\":").unwrap_or(1);
    let end_line = json_u32_after(span, b"\"line_end\":").unwrap_or(line);
    let end_column = json_u32_after(span, b"\"column_end\":").unwrap_or(column);
    let severity = if bytes_find(source, b"\"level\":\"error\"").is_some() {
        DiagnosticSeverity::Error
    } else if bytes_find(source, b"\"level\":\"warning\"").is_some() {
        DiagnosticSeverity::Warning
    } else {
        DiagnosticSeverity::Info
    };
    let message_start = bytes_find(source, b"\"message\":{").and_then(|object| {
        bytes_find(&source[object + 11..], b"\"message\":").map(|position| object + 11 + position)
    });
    let message = message_start
        .and_then(|position| json_string(source, position + b"\"message\":".len()))
        .map_or_else(|| String::from("compiler diagnostic"), |value| value.0);
    let path = std::path::Path::new(&file_name);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let severity_name = match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Info => "info",
    };
    let mut display = format!(
        "{severity_name} {}:{line}:{column} ",
        path.strip_prefix(root).unwrap_or(&path).display()
    );
    let message_start = display.len().min(u32::MAX as usize) as u32;
    display.push_str(&message);
    diagnostics.push(Diagnostic {
        path,
        display,
        message_start,
        line: line.saturating_sub(1),
        column: column.saturating_sub(1),
        end_line: end_line.saturating_sub(1),
        end_column: end_column.saturating_sub(1),
        severity,
    });
}

fn json_object_containing(source: &[u8], target: usize) -> Option<&[u8]> {
    profiling::function_scope!();
    let mut stack = [0usize; 32];
    let mut depth = 0usize;
    let mut position = 0usize;
    let mut string = false;
    let mut escaped = false;
    while position <= target.min(source.len().saturating_sub(1)) {
        let byte = source[position];
        if string {
            if byte == b'\\' && !escaped {
                escaped = true;
            } else {
                if byte == b'"' && !escaped {
                    string = false;
                }
                escaped = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'{' {
            if depth >= stack.len() {
                return None;
            }
            stack[depth] = position;
            depth += 1;
        } else if byte == b'}' {
            depth = depth.saturating_sub(1);
        }
        position += 1;
    }
    if depth == 0 {
        return None;
    }
    let start = stack[depth - 1];
    position = start;
    depth = 0;
    string = false;
    escaped = false;
    while position < source.len() {
        let byte = source[position];
        if string {
            if byte == b'\\' && !escaped {
                escaped = true;
            } else {
                if byte == b'"' && !escaped {
                    string = false;
                }
                escaped = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(&source[start..=position]);
            }
        }
        position += 1;
    }
    None
}

fn json_string(source: &[u8], mut position: usize) -> Option<(String, usize)> {
    while position < source.len() && source[position].is_ascii_whitespace() {
        position += 1;
    }
    if source.get(position) != Some(&b'"') {
        return None;
    }
    position += 1;
    let mut value = Vec::with_capacity(64);
    while position < source.len() {
        let byte = source[position];
        position += 1;
        if byte == b'"' {
            return String::from_utf8(value).ok().map(|value| (value, position));
        }
        if byte != b'\\' {
            value.push(byte);
            continue;
        }
        let Some(&escaped) = source.get(position) else {
            return None;
        };
        position += 1;
        value.push(match escaped {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'"' => b'"',
            b'\\' => b'\\',
            _ => escaped,
        });
    }
    None
}

fn json_u32_after(source: &[u8], key: &[u8]) -> Option<u32> {
    let Some(key_position) = bytes_rfind(source, key) else {
        return None;
    };
    let mut position = key_position + key.len();
    let mut value = 0u32;
    let mut found = false;
    while position < source.len() && source[position].is_ascii_digit() {
        found = true;
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(source[position] - b'0'));
        position += 1;
    }
    found.then_some(value)
}

fn bytes_find(source: &[u8], needle: &[u8]) -> Option<usize> {
    source
        .windows(needle.len())
        .position(|window| window == needle)
}

fn bytes_rfind(source: &[u8], needle: &[u8]) -> Option<usize> {
    source
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_json_primary_span_becomes_severity_sorted_diagnostic() {
        let source = br#"{"reason":"compiler-message","message":{"rendered":"","children":[],"level":"error","message":"mismatched types","spans":[{"byte_end":40,"byte_start":37,"column_end":12,"column_start":9,"file_name":"src/main.rs","is_primary":true,"label":"wrong type","line_end":7,"line_start":7}]}}"#;
        let mut diagnostics = Vec::new();
        diagnostics_parse_line(std::path::Path::new("/work"), source, &mut diagnostics);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(
            diagnostics[0].path,
            std::path::Path::new("/work/src/main.rs")
        );
        assert_eq!((diagnostics[0].line, diagnostics[0].column), (6, 8));
        assert_eq!(
            (diagnostics[0].end_line, diagnostics[0].end_column),
            (6, 11)
        );
        assert_eq!(
            &diagnostics[0].display[diagnostics[0].message_start as usize..],
            "mismatched types"
        );
        assert!(diagnostics[0].display.contains("mismatched types"));
    }
}
