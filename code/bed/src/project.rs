use core::alloc::Allocator;
use std::os::unix::ffi::OsStrExt as _;

const IGNORE_FILE_NAMES: [&str; 4] = [".bedignore", ".editorignore", ".ignore", ".gitignore"];

pub struct IgnoreRule {
    pub pattern: std::ops::Range<usize>,
    pub negated: bool,
    pub directory_only: bool,
    pub anchored: bool,
    pub contains_slash: bool,
}

pub struct ProjectFiles {
    pub root: std::path::PathBuf,
    pub paths: std::sync::Arc<Vec<std::path::PathBuf>>,
    pub labels: std::sync::Arc<Vec<String>>,
    pub ignore_file: Option<std::path::PathBuf>,
}

pub struct ProjectDiscovery {
    pub project: ProjectFiles,
    pub complete: bool,
    pub state: Option<ProjectDiscoveryState>,
}

pub struct ProjectDiscoveryState {
    directories: std::collections::VecDeque<std::path::PathBuf>,
    entries: Option<std::fs::ReadDir>,
    rules: Vec<IgnoreRule>,
    pattern_bytes: Vec<u8>,
}

pub fn project_discover(root: std::path::PathBuf, maximum_entries: usize) -> ProjectDiscovery {
    profiling::function_scope!();
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let ignore_file = IGNORE_FILE_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    let mut rules = Vec::with_capacity(128);
    let mut pattern_bytes = Vec::with_capacity(512);
    if let Some(path) = &ignore_file {
        let source = std::fs::read(path).unwrap_or_default();
        pattern_bytes.reserve(source.len());
        ignore_rules_parse(&source, &mut rules, &mut pattern_bytes);
    }
    let mut directories = std::collections::VecDeque::with_capacity(512);
    directories.push_back(root.clone());
    let mut state = ProjectDiscoveryState {
        directories,
        entries: None,
        rules,
        pattern_bytes,
    };
    let mut project = ProjectFiles {
        root,
        paths: std::sync::Arc::new(Vec::new()),
        labels: std::sync::Arc::new(Vec::new()),
        ignore_file,
    };
    let complete = project_discovery_step(
        &mut project,
        &mut state,
        maximum_entries,
        std::time::Duration::MAX,
    );
    ProjectDiscovery {
        project,
        complete,
        state: (!complete).then_some(state),
    }
}

pub fn project_discovery_step(
    project: &mut ProjectFiles,
    state: &mut ProjectDiscoveryState,
    maximum_entries: usize,
    maximum_time: std::time::Duration,
) -> bool {
    profiling::function_scope!();
    profiling::scope!("walk project files breadth first");
    let started = std::time::Instant::now();
    let mut visited_entries = 0;
    while visited_entries < maximum_entries {
        if started.elapsed() >= maximum_time {
            return false;
        }
        if state.entries.is_none() {
            let Some(directory) = state.directories.pop_front() else {
                return true;
            };
            state.entries = std::fs::read_dir(directory).ok();
            if state.entries.is_none() {
                continue;
            }
        }
        let entry = match state.entries.as_mut().and_then(Iterator::next) {
            Some(Ok(entry)) => entry,
            Some(Err(_)) => {
                visited_entries += 1;
                continue;
            }
            None => {
                state.entries = None;
                continue;
            }
        };
        visited_entries += 1;
        let path = entry.path();
        let relative = path.strip_prefix(&project.root).unwrap_or(&path);
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let name = entry.file_name();
        if (file_type.is_dir() && matches!(name.to_str(), Some(".git" | "target" | "node_modules")))
            || ignore_rules_match(
                &state.rules,
                &state.pattern_bytes,
                relative,
                file_type.is_dir(),
            )
        {
            continue;
        }
        if file_type.is_dir() {
            state.directories.push_back(path);
        } else if file_type.is_file() {
            std::sync::Arc::make_mut(&mut project.labels)
                .push(relative.to_string_lossy().into_owned());
            std::sync::Arc::make_mut(&mut project.paths).push(path);
        }
    }
    state.entries.is_none() && state.directories.is_empty()
}

pub fn ignore_rules_parse(
    source: &[u8],
    rules: &mut Vec<IgnoreRule, impl Allocator>,
    pattern_bytes: &mut Vec<u8, impl Allocator>,
) {
    profiling::function_scope!();
    rules.clear();
    pattern_bytes.clear();
    for raw_line in source.split(|&byte| byte == b'\n') {
        let mut line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        while line.last() == Some(&b' ') && !line.ends_with(b"\\ ") {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let negated = line[0] == b'!';
        if negated {
            line = &line[1..];
        }
        let anchored = line.first() == Some(&b'/');
        if anchored {
            line = &line[1..];
        }
        let directory_only = line.last() == Some(&b'/');
        if directory_only {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() {
            continue;
        }
        let pattern_start = pattern_bytes.len();
        let mut position = 0;
        while position < line.len() {
            if line[position] == b'\\' && position + 1 < line.len() {
                position += 1;
            }
            pattern_bytes.push(line[position]);
            position += 1;
        }
        let pattern_end = pattern_bytes.len();
        let contains_slash = pattern_bytes[pattern_start..pattern_end].contains(&b'/');
        rules.push(IgnoreRule {
            pattern: pattern_start..pattern_end,
            negated,
            directory_only,
            anchored,
            contains_slash,
        });
    }
}

pub fn ignore_rules_match(
    rules: &[IgnoreRule],
    pattern_bytes: &[u8],
    path: &std::path::Path,
    is_directory: bool,
) -> bool {
    profiling::function_scope!();
    let path = path.as_os_str().as_bytes();
    let basename = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    let mut ignored = false;
    for rule in rules {
        if rule.directory_only && !is_directory {
            continue;
        }
        let candidate = if rule.anchored || rule.contains_slash {
            path
        } else {
            basename
        };
        if glob_matches(&pattern_bytes[rule.pattern.clone()], candidate) {
            ignored = !rule.negated;
        }
    }
    ignored
}

pub fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    profiling::function_scope!();
    let mut pattern_position = 0;
    let mut text_position = 0;
    let mut star_pattern = usize::MAX;
    let mut star_text = 0;
    let mut star_crosses_slash = false;
    while text_position < text.len() {
        if pattern_position < pattern.len()
            && pattern[pattern_position] == b'?'
            && text[text_position] != b'/'
        {
            pattern_position += 1;
            text_position += 1;
        } else if pattern_position < pattern.len() && pattern[pattern_position] == b'[' {
            match glob_class_match(&pattern[pattern_position..], text[text_position]) {
                Some((matched, consumed)) if matched && text[text_position] != b'/' => {
                    pattern_position += consumed;
                    text_position += 1;
                }
                _ if star_pattern != usize::MAX
                    && star_text < text.len()
                    && (star_crosses_slash || text[star_text] != b'/') =>
                {
                    star_text += 1;
                    text_position = star_text;
                    pattern_position = star_pattern;
                }
                _ => return false,
            }
        } else if pattern_position < pattern.len()
            && pattern[pattern_position] == text[text_position]
        {
            pattern_position += 1;
            text_position += 1;
        } else if pattern_position < pattern.len() && pattern[pattern_position] == b'*' {
            star_crosses_slash = pattern.get(pattern_position + 1) == Some(&b'*');
            pattern_position += if star_crosses_slash { 2 } else { 1 };
            star_pattern = pattern_position;
            star_text = text_position;
        } else if star_pattern != usize::MAX
            && star_text < text.len()
            && (star_crosses_slash || text[star_text] != b'/')
        {
            star_text += 1;
            text_position = star_text;
            pattern_position = star_pattern;
        } else {
            return false;
        }
    }
    while pattern_position < pattern.len() && pattern[pattern_position] == b'*' {
        pattern_position += 1;
    }
    pattern_position == pattern.len()
}

fn glob_class_match(pattern: &[u8], byte: u8) -> Option<(bool, usize)> {
    let mut position = 1;
    let negated = matches!(pattern.get(position), Some(b'!') | Some(b'^'));
    position += usize::from(negated);
    let mut matched = false;
    while position < pattern.len() && pattern[position] != b']' {
        if position + 2 < pattern.len()
            && pattern[position + 1] == b'-'
            && pattern[position + 2] != b']'
        {
            matched |= (pattern[position]..=pattern[position + 2]).contains(&byte);
            position += 3;
        } else {
            matched |= pattern[position] == byte;
            position += 1;
        }
    }
    if position == pattern.len() {
        return None;
    }
    Some((matched != negated, position + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_supports_segment_and_recursive_wildcards() {
        assert!(glob_matches(b"target/**", b"target/debug/bed"));
        assert!(glob_matches(b"*.rs", b"editor.rs"));
        assert!(!glob_matches(b"*.rs", b"src/editor.rs"));
        assert!(glob_matches(b"file[0-9].rs", b"file7.rs"));
    }

    #[test]
    fn later_negation_reincludes_a_path() {
        let mut rules = Vec::new();
        let mut pattern_bytes = Vec::new();
        ignore_rules_parse(b"*.rs\n!keep.rs\n", &mut rules, &mut pattern_bytes);
        assert!(ignore_rules_match(
            &rules,
            &pattern_bytes,
            std::path::Path::new("skip.rs"),
            false
        ));
        assert!(!ignore_rules_match(
            &rules,
            &pattern_bytes,
            std::path::Path::new("keep.rs"),
            false
        ));
    }

    #[test]
    fn first_ignore_file_is_authoritative() {
        let root = std::env::temp_dir().join(format!("bed-project-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("keep.rs"), "").unwrap();
        std::fs::write(root.join("skip.rs"), "").unwrap();
        std::fs::write(root.join(".bedignore"), "skip.rs\n").unwrap();
        std::fs::write(root.join(".gitignore"), "keep.rs\n").unwrap();
        let project = project_discover(root.clone(), usize::MAX).project;
        assert!(project.labels.iter().any(|path| path == "keep.rs"));
        assert!(!project.labels.iter().any(|path| path == "skip.rs"));
        assert_eq!(
            project.ignore_file.as_deref(),
            Some(root.join(".bedignore").as_path())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_discovery_continues_breadth_first() {
        let root =
            std::env::temp_dir().join(format!("bed-project-breadth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("one/deep")).unwrap();
        std::fs::create_dir_all(root.join("two/deep")).unwrap();
        std::fs::write(root.join("one/file.rs"), "").unwrap();
        std::fs::write(root.join("two/file.rs"), "").unwrap();

        let mut discovery = project_discover(root.clone(), 2);
        assert!(!discovery.complete);
        let mut state = discovery.state.take().unwrap();
        assert!(!project_discovery_step(
            &mut discovery.project,
            &mut state,
            2,
            std::time::Duration::MAX
        ));
        assert!(!project_discovery_step(
            &mut discovery.project,
            &mut state,
            2,
            std::time::Duration::MAX
        ));
        assert!(
            discovery
                .project
                .labels
                .iter()
                .any(|label| label.starts_with("one/"))
        );
        assert!(
            discovery
                .project
                .labels
                .iter()
                .any(|label| label.starts_with("two/"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_repository_metadata_is_not_discovered() {
        let root = std::env::temp_dir().join(format!(
            "bed-project-nested-git-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/.git/objects/aa")).unwrap();
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::write(root.join("repo/.git/objects/aa/object"), "object").unwrap();
        std::fs::write(root.join("repo/src/main.rs"), "fn main() {}").unwrap();

        let project = project_discover(root.clone(), usize::MAX).project;
        assert!(
            project
                .labels
                .iter()
                .any(|label| label == "repo/src/main.rs")
        );
        assert!(project.labels.iter().all(|label| !label.contains("/.git/")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
