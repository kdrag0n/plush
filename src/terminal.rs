use std::io::{self, Write};

pub fn repair_terminal() {
    let mut out = io::stdout();
    let _ = write!(
        out,
        "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b[?25h\x1b[?1049l\x1b[?47l\x1b[?1047l"
    );
    let _ = out.flush();
}
