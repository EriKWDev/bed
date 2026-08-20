use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Alt(char),
    Control(u8),
    Escape,
    Enter,
    Backspace,
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Delete,
}

pub struct Terminal {
    pub device: std::fs::File,
    #[cfg(unix)]
    pub original: libc::termios,
    #[cfg(unix)]
    previous_panic_hook: Option<SharedPanicHook>,
}

#[cfg(unix)]
type SharedPanicHook =
    std::sync::Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

pub fn terminal_open() -> std::io::Result<Terminal> {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd;

        let device = match open_terminal_device() {
            Ok(device) => device,
            Err(error) => return Err(error),
        };
        let descriptor = device.as_raw_fd();

        let mut original = std::mem::zeroed();
        if libc::tcgetattr(descriptor, &mut original) == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let mut raw = original;
        libc::cfmakeraw(&mut raw);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(descriptor, libc::TCSAFLUSH, &raw) == -1 {
            return Err(std::io::Error::last_os_error());
        }

        let previous_panic_hook: SharedPanicHook = std::panic::take_hook().into();
        let panic_hook = std::sync::Arc::clone(&previous_panic_hook);
        std::panic::set_hook(Box::new(move |information| {
            terminal_restore_descriptor(descriptor, &original);
            panic_hook(information);
        }));
        let mut terminal = Terminal {
            device,
            original,
            previous_panic_hook: Some(previous_panic_hook),
        };
        if let Err(error) = terminal.device.write_all(b"\x1b[?1049h\x1b[?25l") {
            return Err(error);
        }
        if let Err(error) = terminal.device.flush() {
            return Err(error);
        }
        Ok(terminal)
    }

    #[cfg(not(unix))]
    {
        Err(std::io::Error::other(
            "direct terminal mode is currently supported on Unix",
        ))
    }
}

#[cfg(unix)]
fn open_terminal_device() -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;

    for descriptor in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::isatty(descriptor) } != 1 {
            continue;
        }

        let mut path_bytes = [0 as libc::c_char; 256];
        let result =
            unsafe { libc::ttyname_r(descriptor, path_bytes.as_mut_ptr(), path_bytes.len()) };
        if result != 0 {
            continue;
        }
        let path = unsafe { std::ffi::CStr::from_ptr(path_bytes.as_ptr()) };
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(path.to_bytes()));
        if let Ok(device) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            return Ok(device);
        }
    }

    Err(std::io::Error::other("no terminal device is available"))
}

pub fn terminal_size(terminal: &Terminal) -> (usize, usize) {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd;

        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(terminal.device.as_raw_fd(), libc::TIOCGWINSZ, &mut size) == 0 {
            return (
                usize::from(size.ws_col).max(1),
                usize::from(size.ws_row).max(2),
            );
        }
    }
    (80, 24)
}

pub fn terminal_read_key(terminal: &mut Terminal) -> std::io::Result<Key> {
    let first = match read_byte(&mut terminal.device) {
        Ok(first) => first,
        Err(error) => return Err(error),
    };
    match first {
        b'\r' | b'\n' => Ok(Key::Enter),
        b'\t' => Ok(Key::Tab),
        0x7f => Ok(Key::Backspace),
        0x1b => read_escape_sequence(&mut terminal.device),
        0x00..=0x1f => Ok(Key::Control(first)),
        0x20..=0x7e => Ok(Key::Character(first as char)),
        _ => read_utf8_character(&mut terminal.device, first),
    }
}

pub fn terminal_read_key_timeout(
    terminal: &mut Terminal,
    timeout_milliseconds: i32,
) -> std::io::Result<Option<Key>> {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd;

        let mut descriptor = libc::pollfd {
            fd: terminal.device.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = libc::poll(&mut descriptor, 1, timeout_milliseconds);
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if ready == 0 {
            return Ok(None);
        }
    }

    match terminal_read_key(terminal) {
        Ok(key) => Ok(Some(key)),
        Err(error) => Err(error),
    }
}

pub fn terminal_present(terminal: &mut Terminal, frame: &[u8]) -> std::io::Result<()> {
    if let Err(error) = terminal.device.write_all(frame) {
        return Err(error);
    }
    terminal.device.flush()
}

pub fn terminal_character_width(character: char) -> usize {
    let codepoint = character as u32;
    if codepoint == 0
        || codepoint < 0x20
        || (0x7f..0xa0).contains(&codepoint)
        || matches!(
            codepoint,
            0x0300..=0x036f
                | 0x0483..=0x0489
                | 0x0591..=0x05bd
                | 0x05bf
                | 0x05c1..=0x05c2
                | 0x05c4..=0x05c5
                | 0x0610..=0x061a
                | 0x064b..=0x065f
                | 0x0670
                | 0x06d6..=0x06ed
                | 0x1ab0..=0x1aff
                | 0x1dc0..=0x1dff
                | 0x200b..=0x200f
                | 0x202a..=0x202e
                | 0x2060..=0x206f
                | 0x20d0..=0x20ff
                | 0xfe00..=0xfe0f
                | 0xfe20..=0xfe2f
                | 0xe0100..=0xe01ef
        )
    {
        return 0;
    }
    if codepoint >= 0x1100
        && (codepoint <= 0x115f
            || matches!(codepoint, 0x2329 | 0x232a)
            || (0x2e80..=0xa4cf).contains(&codepoint) && codepoint != 0x303f
            || (0xac00..=0xd7a3).contains(&codepoint)
            || (0xf900..=0xfaff).contains(&codepoint)
            || (0xfe10..=0xfe19).contains(&codepoint)
            || (0xfe30..=0xfe6f).contains(&codepoint)
            || (0xff00..=0xff60).contains(&codepoint)
            || (0xffe0..=0xffe6).contains(&codepoint)
            || (0x1f300..=0x1faff).contains(&codepoint)
            || (0x20000..=0x3fffd).contains(&codepoint))
    {
        return 2;
    }
    1
}

fn read_byte(input: &mut std::fs::File) -> std::io::Result<u8> {
    let mut byte = [0];
    if let Err(error) = input.read_exact(&mut byte) {
        return Err(error);
    }
    Ok(byte[0])
}

fn read_escape_sequence(input: &mut std::fs::File) -> std::io::Result<Key> {
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd;

        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if libc::poll(&mut descriptor, 1, 10) <= 0 {
            return Ok(Key::Escape);
        }
    }

    let second = match read_byte(input) {
        Ok(second) => second,
        Err(error) => return Err(error),
    };
    if second != b'[' {
        return Ok(if second.is_ascii() {
            Key::Alt(second as char)
        } else {
            Key::Escape
        });
    }
    let third = match read_byte(input) {
        Ok(third) => third,
        Err(error) => return Err(error),
    };
    Ok(match third {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'Z' => Key::BackTab,
        b'3' => {
            let _tilde = match read_byte(input) {
                Ok(tilde) => tilde,
                Err(error) => return Err(error),
            };
            Key::Delete
        }
        _ => Key::Escape,
    })
}

fn read_utf8_character(input: &mut std::fs::File, first: u8) -> std::io::Result<Key> {
    let length = if first & 0b1110_0000 == 0b1100_0000 {
        2
    } else if first & 0b1111_0000 == 0b1110_0000 {
        3
    } else if first & 0b1111_1000 == 0b1111_0000 {
        4
    } else {
        return Ok(Key::Character('\u{fffd}'));
    };

    let mut bytes = [0; 4];
    bytes[0] = first;
    if let Err(error) = input.read_exact(&mut bytes[1..length]) {
        return Err(error);
    }
    let character = std::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(|text| text.chars().next())
        .unwrap_or('\u{fffd}');
    Ok(Key::Character(character))
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self
            .device
            .write_all(b"\x1b[0m\x1b[0 q\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = self.device.flush();
        #[cfg(unix)]
        unsafe {
            use std::os::fd::AsRawFd;

            libc::tcsetattr(self.device.as_raw_fd(), libc::TCSAFLUSH, &self.original);
        }
        #[cfg(unix)]
        if !std::thread::panicking()
            && let Some(previous) = self.previous_panic_hook.take()
        {
            std::panic::set_hook(Box::new(move |information| previous(information)));
        }
    }
}

#[cfg(unix)]
fn terminal_restore_descriptor(descriptor: std::os::fd::RawFd, original: &libc::termios) {
    const RESTORE: &[u8] = b"\x1b[0m\x1b[0 q\x1b[?7h\x1b[?25h\x1b[?1049l";
    unsafe {
        libc::write(
            descriptor,
            RESTORE.as_ptr().cast::<libc::c_void>(),
            RESTORE.len(),
        );
        libc::tcsetattr(descriptor, libc::TCSAFLUSH, original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_width_distinguishes_combining_and_wide_characters() {
        assert_eq!(terminal_character_width('a'), 1);
        assert_eq!(terminal_character_width('\u{0301}'), 0);
        assert_eq!(terminal_character_width('界'), 2);
        assert_eq!(terminal_character_width('🦀'), 2);
    }
}
