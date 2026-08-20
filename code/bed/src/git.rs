use crate::buffer::{GapBuffer, buffer_byte, buffer_len};

const GIT_DIFF_LOOKAHEAD_LINES: usize = 64;
const GIT_LINE_ADDED: u8 = 1 << 0;
const GIT_LINE_MODIFIED: u8 = 1 << 1;
const GIT_LINE_REMOVED: u8 = 1 << 2;
const LINE_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const LINE_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitLine {
    pub start: u32,
    pub end: u32,
    pub hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitGutterLine {
    pub line: u32,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitGutterPhase {
    Disabled,
    WaitingForGit,
    BaselineLines,
    CurrentLines,
    Diff,
    Complete,
}

pub struct GitBaseline {
    pub bytes: Vec<u8>,
    pub available: bool,
}

pub struct GitGutter {
    pub task: Option<idno_std::micropool::OwnedTask<GitBaseline>>,
    pub baseline: Vec<u8>,
    pub baseline_lines: Vec<GitLine>,
    pub current_lines: Vec<GitLine>,
    pub markers: Vec<GitGutterLine>,
    pub previous_markers: Vec<GitGutterLine>,
    pub phase: GitGutterPhase,
    pub byte_position: usize,
    pub line_start: usize,
    pub line_hash: u64,
    pub baseline_line: usize,
    pub current_line: usize,
}

pub fn git_gutter_empty() -> GitGutter {
    GitGutter {
        task: None,
        baseline: Vec::new(),
        baseline_lines: Vec::new(),
        current_lines: Vec::new(),
        markers: Vec::new(),
        previous_markers: Vec::new(),
        phase: GitGutterPhase::Disabled,
        byte_position: 0,
        line_start: 0,
        line_hash: LINE_HASH_OFFSET,
        baseline_line: 0,
        current_line: 0,
    }
}

pub fn git_gutter_set_path(gutter: &mut GitGutter, path: Option<&std::path::Path>) {
    profiling::function_scope!();
    if let Some(task) = gutter.task.take() {
        task.cancel();
    }
    git_gutter_clear(gutter);
    let Some(path) = path else {
        return;
    };
    let path = path.to_path_buf();
    gutter.task = Some(idno_std::threads().spawn_owned(move || git_baseline_load(&path)));
    gutter.phase = GitGutterPhase::WaitingForGit;
}

pub fn git_gutter_clear(gutter: &mut GitGutter) {
    profiling::function_scope!();
    let GitGutter {
        task: _,
        baseline,
        baseline_lines,
        current_lines,
        markers,
        previous_markers,
        phase,
        byte_position,
        line_start,
        line_hash,
        baseline_line,
        current_line,
    } = gutter;
    baseline.clear();
    baseline_lines.clear();
    current_lines.clear();
    markers.clear();
    previous_markers.clear();
    *phase = GitGutterPhase::Disabled;
    *byte_position = 0;
    *line_start = 0;
    *line_hash = LINE_HASH_OFFSET;
    *baseline_line = 0;
    *current_line = 0;
}

pub fn git_gutter_invalidate(gutter: &mut GitGutter) {
    profiling::function_scope!();
    if matches!(
        gutter.phase,
        GitGutterPhase::Disabled | GitGutterPhase::WaitingForGit
    ) {
        return;
    }
    gutter.current_lines.clear();
    if gutter.previous_markers.is_empty() && !gutter.markers.is_empty() {
        std::mem::swap(&mut gutter.markers, &mut gutter.previous_markers);
    }
    gutter.markers.clear();
    gutter.phase = GitGutterPhase::CurrentLines;
    gutter.byte_position = 0;
    gutter.line_start = 0;
    gutter.line_hash = LINE_HASH_OFFSET;
    gutter.baseline_line = 0;
    gutter.current_line = 0;
}

pub fn git_gutter_poll(gutter: &mut GitGutter) -> bool {
    profiling::function_scope!();
    let complete = gutter
        .task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let Some(task) = gutter.task.take() else {
        return false;
    };
    let baseline = match task.try_join() {
        Ok(baseline) => baseline,
        Err(task) => {
            gutter.task = Some(task);
            return false;
        }
    };
    if !baseline.available {
        gutter.phase = GitGutterPhase::Disabled;
        return false;
    }
    gutter.baseline = baseline.bytes;
    gutter.baseline_lines.clear();
    gutter.current_lines.clear();
    gutter.markers.clear();
    gutter.phase = GitGutterPhase::BaselineLines;
    gutter.byte_position = 0;
    gutter.line_start = 0;
    gutter.line_hash = LINE_HASH_OFFSET;
    true
}

pub fn git_gutter_step(
    buffer: &GapBuffer,
    gutter: &mut GitGutter,
    maximum_bytes: usize,
    maximum_time: std::time::Duration,
) -> bool {
    profiling::function_scope!();
    let original_marker_count = gutter.markers.len();
    let original_phase = gutter.phase;
    let retained_previous = !gutter.previous_markers.is_empty();
    let started = std::time::Instant::now();
    match gutter.phase {
        GitGutterPhase::BaselineLines => {
            git_gutter_build_baseline_lines(gutter, maximum_bytes, maximum_time, started)
        }
        GitGutterPhase::CurrentLines => {
            git_gutter_build_current_lines(buffer, gutter, maximum_bytes, maximum_time, started)
        }
        GitGutterPhase::Diff => git_gutter_diff(buffer, gutter, maximum_time, started),
        GitGutterPhase::Disabled | GitGutterPhase::WaitingForGit | GitGutterPhase::Complete => {}
    }
    if gutter.phase == GitGutterPhase::Complete && original_phase != GitGutterPhase::Complete {
        gutter.previous_markers.clear();
        true
    } else {
        !retained_previous
            && (gutter.markers.len() != original_marker_count || gutter.phase != original_phase)
    }
}

pub fn git_gutter_pending(gutter: &GitGutter) -> bool {
    gutter.task.is_some()
        || !matches!(
            gutter.phase,
            GitGutterPhase::Disabled | GitGutterPhase::Complete
        )
}

pub fn git_gutter_flags(gutter: &GitGutter, line: usize) -> u8 {
    let markers = git_gutter_visible_markers(gutter);
    let position = markers.partition_point(|marker| marker.line < line as u32);
    markers
        .get(position)
        .filter(|marker| marker.line == line as u32)
        .map_or(0, |marker| marker.flags)
}

pub fn git_gutter_next_change(gutter: &GitGutter, line: usize, forward: bool) -> Option<usize> {
    profiling::function_scope!();
    let markers = git_gutter_visible_markers(gutter);
    if markers.is_empty() {
        return None;
    }
    let marker = if forward {
        let position = markers.partition_point(|marker| marker.line <= line as u32);
        &markers[position % markers.len()]
    } else {
        let position = markers.partition_point(|marker| marker.line < line as u32);
        &markers[position.checked_sub(1).unwrap_or(markers.len() - 1)]
    };
    Some(marker.line as usize)
}

pub fn git_gutter_adjust_edits(gutter: &mut GitGutter, edits: &[(usize, usize, usize)]) {
    profiling::function_scope!();
    let markers = if !gutter.previous_markers.is_empty() {
        &mut gutter.previous_markers
    } else {
        &mut gutter.markers
    };
    let mut shift = 0isize;
    for &(original_start_line, original_end_line, inserted_lines) in edits {
        let start_line = (original_start_line as isize + shift) as usize;
        let end_line = (original_end_line as isize + shift) as usize;
        let delta = inserted_lines as isize - (original_end_line - original_start_line) as isize;
        let mut retained_flags = 0;
        markers.retain_mut(|marker| {
            let line = marker.line as usize;
            if line < start_line {
                true
            } else if line > end_line {
                marker.line = (line as isize + delta) as u32;
                true
            } else {
                retained_flags |= marker.flags;
                false
            }
        });
        let provisional_flags = if retained_flags & GIT_LINE_ADDED != 0 {
            GIT_LINE_ADDED
        } else {
            retained_flags | GIT_LINE_MODIFIED
        };
        let position = markers.partition_point(|marker| marker.line < start_line as u32);
        if markers
            .get(position)
            .is_some_and(|marker| marker.line == start_line as u32)
        {
            markers[position].flags |= provisional_flags;
        } else {
            markers.insert(
                position,
                GitGutterLine {
                    line: start_line as u32,
                    flags: provisional_flags,
                },
            );
        }
        shift += delta;
    }
}

fn git_gutter_visible_markers(gutter: &GitGutter) -> &[GitGutterLine] {
    if gutter.phase != GitGutterPhase::Complete && !gutter.previous_markers.is_empty() {
        &gutter.previous_markers
    } else {
        &gutter.markers
    }
}

pub fn git_gutter_line_added(flags: u8) -> bool {
    flags & GIT_LINE_ADDED != 0
}

pub fn git_gutter_line_modified(flags: u8) -> bool {
    flags & GIT_LINE_MODIFIED != 0
}

pub fn git_gutter_line_removed(flags: u8) -> bool {
    flags & GIT_LINE_REMOVED != 0
}

fn git_baseline_load(path: &std::path::Path) -> GitBaseline {
    profiling::function_scope!();
    let Some(parent) = path.parent() else {
        return GitBaseline {
            bytes: Vec::new(),
            available: false,
        };
    };
    let repository = std::process::Command::new("git")
        .arg("-C")
        .arg(parent)
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    let repository = match repository {
        Ok(repository) if repository.status.success() => repository,
        _ => {
            return GitBaseline {
                bytes: Vec::new(),
                available: false,
            };
        }
    };
    let root = std::path::PathBuf::from(std::ffi::OsStr::new(
        std::str::from_utf8(&repository.stdout)
            .unwrap_or("")
            .trim_end(),
    ));
    let relative = path.strip_prefix(&root).unwrap_or(path);
    let mut object = std::ffi::OsString::from("HEAD:");
    object.push(relative);
    let baseline = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["--no-pager", "show", "--no-textconv"])
        .arg(object)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    match baseline {
        Ok(baseline) if baseline.status.success() => GitBaseline {
            bytes: baseline.stdout,
            available: true,
        },
        _ => GitBaseline {
            bytes: Vec::new(),
            available: true,
        },
    }
}

fn git_gutter_build_baseline_lines(
    gutter: &mut GitGutter,
    maximum_bytes: usize,
    maximum_time: std::time::Duration,
    started: std::time::Instant,
) {
    profiling::function_scope!();
    let end = gutter
        .byte_position
        .saturating_add(maximum_bytes)
        .min(gutter.baseline.len());
    while gutter.byte_position < end {
        if gutter.byte_position & 255 == 0 && started.elapsed() >= maximum_time {
            return;
        }
        let byte = gutter.baseline[gutter.byte_position];
        if byte == b'\n' {
            git_line_push(
                &mut gutter.baseline_lines,
                gutter.line_start,
                gutter.byte_position,
                gutter.line_hash,
            );
            gutter.byte_position += 1;
            gutter.line_start = gutter.byte_position;
            gutter.line_hash = LINE_HASH_OFFSET;
        } else {
            gutter.line_hash = (gutter.line_hash ^ u64::from(byte)).wrapping_mul(LINE_HASH_PRIME);
            gutter.byte_position += 1;
        }
    }
    if gutter.byte_position == gutter.baseline.len() {
        if gutter.line_start < gutter.byte_position {
            git_line_push(
                &mut gutter.baseline_lines,
                gutter.line_start,
                gutter.byte_position,
                gutter.line_hash,
            );
        }
        gutter.phase = GitGutterPhase::CurrentLines;
        gutter.byte_position = 0;
        gutter.line_start = 0;
        gutter.line_hash = LINE_HASH_OFFSET;
    }
}

fn git_gutter_build_current_lines(
    buffer: &GapBuffer,
    gutter: &mut GitGutter,
    maximum_bytes: usize,
    maximum_time: std::time::Duration,
    started: std::time::Instant,
) {
    profiling::function_scope!();
    let length = buffer_len(buffer).min(u32::MAX as usize);
    let end = gutter
        .byte_position
        .saturating_add(maximum_bytes)
        .min(length);
    while gutter.byte_position < end {
        if gutter.byte_position & 255 == 0 && started.elapsed() >= maximum_time {
            return;
        }
        let byte = buffer_byte(buffer, gutter.byte_position);
        if byte == b'\n' {
            git_line_push(
                &mut gutter.current_lines,
                gutter.line_start,
                gutter.byte_position,
                gutter.line_hash,
            );
            gutter.byte_position += 1;
            gutter.line_start = gutter.byte_position;
            gutter.line_hash = LINE_HASH_OFFSET;
        } else {
            gutter.line_hash = (gutter.line_hash ^ u64::from(byte)).wrapping_mul(LINE_HASH_PRIME);
            gutter.byte_position += 1;
        }
    }
    if gutter.byte_position == length {
        if gutter.line_start < gutter.byte_position {
            git_line_push(
                &mut gutter.current_lines,
                gutter.line_start,
                gutter.byte_position,
                gutter.line_hash,
            );
        }
        gutter.phase = GitGutterPhase::Diff;
        gutter.baseline_line = 0;
        gutter.current_line = 0;
    }
}

fn git_gutter_diff(
    buffer: &GapBuffer,
    gutter: &mut GitGutter,
    maximum_time: std::time::Duration,
    started: std::time::Instant,
) {
    profiling::function_scope!();
    profiling::scope!("align git lines");
    while gutter.baseline_line < gutter.baseline_lines.len()
        || gutter.current_line < gutter.current_lines.len()
    {
        if started.elapsed() >= maximum_time {
            return;
        }
        if git_lines_equal(buffer, gutter, gutter.baseline_line, gutter.current_line) {
            gutter.baseline_line += 1;
            gutter.current_line += 1;
            continue;
        }
        let alignment = git_next_alignment(buffer, gutter);
        let (removed, added) = alignment.unwrap_or_else(|| {
            (
                usize::from(gutter.baseline_line < gutter.baseline_lines.len()),
                usize::from(gutter.current_line < gutter.current_lines.len()),
            )
        });
        let modified = removed.min(added);
        for line in gutter.current_line..gutter.current_line + modified {
            git_gutter_mark(gutter, line, GIT_LINE_MODIFIED);
        }
        for line in gutter.current_line + modified..gutter.current_line + added {
            git_gutter_mark(gutter, line, GIT_LINE_ADDED);
        }
        if removed > added {
            git_gutter_mark(gutter, gutter.current_line + added, GIT_LINE_REMOVED);
        }
        gutter.baseline_line += removed;
        gutter.current_line += added;
    }
    gutter.phase = GitGutterPhase::Complete;
}

fn git_next_alignment(buffer: &GapBuffer, gutter: &GitGutter) -> Option<(usize, usize)> {
    let remaining_baseline = gutter.baseline_lines.len() - gutter.baseline_line;
    let remaining_current = gutter.current_lines.len() - gutter.current_line;
    let maximum_distance = GIT_DIFF_LOOKAHEAD_LINES.min(remaining_baseline + remaining_current);
    for distance in 1..=maximum_distance {
        let removed_start = distance.saturating_sub(remaining_current);
        let removed_end = distance.min(remaining_baseline);
        for removed in removed_start..=removed_end {
            let added = distance - removed;
            if removed > remaining_baseline || added > remaining_current {
                continue;
            }
            if git_lines_equal(
                buffer,
                gutter,
                gutter.baseline_line + removed,
                gutter.current_line + added,
            ) {
                return Some((removed, added));
            }
        }
    }
    None
}

fn git_lines_equal(
    buffer: &GapBuffer,
    gutter: &GitGutter,
    baseline_line: usize,
    current_line: usize,
) -> bool {
    let Some(baseline) = gutter.baseline_lines.get(baseline_line) else {
        return false;
    };
    let Some(current) = gutter.current_lines.get(current_line) else {
        return false;
    };
    if baseline.hash != current.hash || baseline.end - baseline.start != current.end - current.start
    {
        return false;
    }
    (0..(baseline.end - baseline.start) as usize).all(|offset| {
        gutter.baseline[baseline.start as usize + offset]
            == buffer_byte(buffer, current.start as usize + offset)
    })
}

fn git_line_push(lines: &mut Vec<GitLine>, start: usize, end: usize, hash: u64) {
    if end > u32::MAX as usize {
        return;
    }
    lines.push(GitLine {
        start: start as u32,
        end: end as u32,
        hash,
    });
}

fn git_gutter_mark(gutter: &mut GitGutter, line: usize, flag: u8) {
    if line > u32::MAX as usize {
        return;
    }
    if let Some(last) = gutter.markers.last_mut()
        && last.line == line as u32
    {
        last.flags |= flag;
        return;
    }
    gutter.markers.push(GitGutterLine {
        line: line as u32,
        flags: flag,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::buffer_from_bytes;

    fn diff(baseline: &[u8], current: &[u8]) -> GitGutter {
        let buffer = buffer_from_bytes(current);
        let mut gutter = git_gutter_empty();
        gutter.baseline.extend_from_slice(baseline);
        gutter.phase = GitGutterPhase::BaselineLines;
        while gutter.phase != GitGutterPhase::Complete {
            git_gutter_step(&buffer, &mut gutter, 16, std::time::Duration::MAX);
        }
        gutter
    }

    #[test]
    fn line_diff_marks_additions_changes_and_removals() {
        let gutter = diff(b"one\ntwo\nthree\nfour\n", b"one\nchanged\nadded\nfour\n");
        assert!(git_gutter_line_modified(git_gutter_flags(&gutter, 1)));
        assert!(git_gutter_line_modified(git_gutter_flags(&gutter, 2)));

        let additions = diff(b"one\nthree\n", b"one\ntwo\nthree\n");
        assert!(git_gutter_line_added(git_gutter_flags(&additions, 1)));

        let removals = diff(b"one\ntwo\nthree\n", b"one\nthree\n");
        assert!(git_gutter_line_removed(git_gutter_flags(&removals, 1)));
    }

    #[test]
    fn change_navigation_wraps_in_both_directions() {
        let mut gutter = git_gutter_empty();
        gutter.markers.extend([
            GitGutterLine {
                line: 2,
                flags: GIT_LINE_ADDED,
            },
            GitGutterLine {
                line: 7,
                flags: GIT_LINE_MODIFIED,
            },
        ]);
        assert_eq!(git_gutter_next_change(&gutter, 2, true), Some(7));
        assert_eq!(git_gutter_next_change(&gutter, 7, true), Some(2));
        assert_eq!(git_gutter_next_change(&gutter, 7, false), Some(2));
        assert_eq!(git_gutter_next_change(&gutter, 2, false), Some(7));
    }

    #[test]
    fn invalidation_keeps_published_gutter_until_diff_finishes() {
        let mut gutter = git_gutter_empty();
        gutter.phase = GitGutterPhase::Complete;
        gutter.markers.push(GitGutterLine {
            line: 3,
            flags: GIT_LINE_MODIFIED,
        });
        git_gutter_invalidate(&mut gutter);
        assert!(git_gutter_line_modified(git_gutter_flags(&gutter, 3)));
        assert!(gutter.markers.is_empty());
        assert_eq!(gutter.previous_markers.len(), 1);
        git_gutter_adjust_edits(&mut gutter, &[(3, 3, 0)]);
        assert!(git_gutter_line_modified(git_gutter_flags(&gutter, 3)));
    }

    #[test]
    fn editing_a_clean_line_publishes_a_provisional_modified_marker() {
        let mut gutter = git_gutter_empty();
        gutter.phase = GitGutterPhase::Complete;
        git_gutter_invalidate(&mut gutter);
        git_gutter_adjust_edits(&mut gutter, &[(2, 2, 0)]);
        assert!(git_gutter_line_modified(git_gutter_flags(&gutter, 2)));
    }
}
