//! End-to-end check of the `Printer` text layer on a real terminal (WBS WP-00 acceptance "line buffering on a
//! pty"; 02 §13, ARCHITECTURE §3.9): the TTY decision is `isatty` on a real pseudo-terminal slave, and a
//! `py_print` line reaches the terminal at once, before `finish()`; on a real pipe the same line stays pending
//! until `finish()`. The unit tests in `src/output.rs` cover the byte-exact ordering over in-memory sinks.
//!
//! Owner: WP-00 (integration lead).
#![cfg(unix)]

#[cfg(test)]
mod pty {
    use std::fs::File;
    use std::io::{self, PipeReader, Read};
    use std::os::fd::AsFd;
    use std::time::{Duration, Instant};

    use rustix::fs::{Mode, OFlags, fcntl_getfl, fcntl_setfl, open};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::isatty;
    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::output::{Printer, PrinterOpts};

    /// A pseudo-terminal pair: (master, slave), the master non-blocking.
    fn pty_pair() -> (File, File) {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY,
            Mode::empty(),
        )
        .unwrap();
        nonblocking(&master);
        (File::from(master), File::from(slave))
    }

    fn nonblocking(fd: impl AsFd) {
        let flags = fcntl_getfl(&fd).unwrap();
        fcntl_setfl(&fd, flags | OFlags::NONBLOCK).unwrap();
    }

    /// What `r` yields within `wait`, stopping early at EOF or once `want` bytes arrived (polling a non-blocking
    /// fd: macOS `poll(2)` rejects ttys).
    fn drain(r: &mut impl Read, want: usize, wait: Duration) -> Vec<u8> {
        let deadline = Instant::now() + wait;
        let mut got = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            match r.read(&mut buf) {
                Ok(0) => return got,
                Ok(n) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.len() >= want {
                        return got;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return got;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read: {e}"),
            }
        }
    }

    fn opts(stdout_is_tty: bool) -> PrinterOpts {
        PrinterOpts {
            brand: Brand::PreCommit,
            color: false,
            stdout_is_tty,
            windows_text_mode: false,
            cancel: CancelToken::new(),
        }
    }

    #[test]
    fn py_print_is_line_buffered_on_a_real_pty() {
        let (mut master, slave) = pty_pair();
        let tty = isatty(&slave);
        assert!(tty, "the pty slave is a terminal");
        let p = Printer::new(Box::new(slave), Box::new(io::sink()), opts(tty));
        p.py_print("==> File .pre-commit-config.yaml");
        // The terminal shows the line before any later write or finish() (ONLCR turns \n into \r\n).
        let want: &[u8] = b"==> File .pre-commit-config.yaml\r\n";
        assert_eq!(
            drain(&mut master, want.len(), Duration::from_secs(10)),
            want
        );
        p.finish(); // nothing was left pending (the slave stays open: a closed slave reads as EIO on Linux)
        assert_eq!(drain(&mut master, 1, Duration::from_millis(100)), b"");
    }

    #[test]
    fn py_print_stays_pending_on_a_real_pipe() {
        let (mut reader, writer): (PipeReader, _) = io::pipe().unwrap();
        nonblocking(&reader);
        let tty = isatty(&writer);
        assert!(!tty, "a pipe is not a terminal");
        let p = Printer::new(Box::new(writer), Box::new(io::sink()), opts(tty));
        p.py_print("==> File .pre-commit-config.yaml");
        assert_eq!(drain(&mut reader, 1, Duration::from_millis(200)), b"");
        p.finish();
        drop(p); // closes the write end: the read below ends at EOF
        assert_eq!(
            drain(&mut reader, usize::MAX, Duration::from_secs(10)),
            b"==> File .pre-commit-config.yaml\n"
        );
    }
}
