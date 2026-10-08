//! S14 / ADR-066: a panic injected while a worker holds the Printer lock does not hang — the panic
//! hook only records (try-lock slot) and cancels — and the Printer keeps working afterwards
//! (poison recovery). Run with `--features fault-injection`.
#![cfg(feature = "fault-injection")]

#[cfg(test)]
mod fault_injection {
    use std::io::{self, Write};
    use std::panic;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use snp_core::brand::Brand;
    use snp_core::cancel::{CancelReason, CancelToken};
    use snp_core::env::Env;
    use snp_core::fault::{self, Point};
    use snp_core::output::{Printer, PrinterOpts};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            snp_core::sync::lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn panic_inside_the_printer_lock_does_not_hang() {
        let token = CancelToken::new();
        // The panic hook of `snp::entry`: record into a try-lock slot, cancel, never lock anything else.
        let slot: Arc<Mutex<Option<String>>> = Arc::default();
        let (hook_token, hook_slot) = (token.clone(), Arc::clone(&slot));
        panic::set_hook(Box::new(move |info| {
            if let Ok(mut s) = hook_slot.try_lock() {
                *s = Some(info.payload_as_str().unwrap_or_default().to_owned());
            }
            hook_token.cancel(CancelReason::Panic);
        }));

        let out = Shared::default();
        let opts = PrinterOpts {
            brand: Brand::Snp,
            color: false,
            stdout_is_tty: false,
            windows_text_mode: false,
            cancel: token.clone(),
        };
        let printer = Printer::new(Box::new(out.clone()), Box::new(io::sink()), opts);

        fault::arm_from_env(&Env::from_pairs([("SNP_FAULT", "PrinterWrite@2")]));
        let (tx, rx) = mpsc::channel();
        let worker_printer = printer.clone();
        std::thread::Builder::new()
            .spawn(move || {
                worker_printer.write_line(b"first"); // hit 1: armed for the 2nd
                let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    worker_printer.write_line(b"boom");
                }));
                tx.send(r.is_err()).unwrap();
            })
            .unwrap();
        let panicked = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker must not hang");
        assert!(panicked);
        assert_eq!(token.reason(), Some(CancelReason::Panic));
        assert_eq!(
            slot.lock().unwrap().as_deref(),
            Some("snp fault injection: PrinterWrite")
        );

        // Poison recovery: the main thread can still print (the crash banner path).
        fault::arm_from_env(&Env::from_pairs::<&str, &str>([]));
        printer.write_line(b"after");
        fault::hit(Point::PrinterWrite);
        drop(panic::take_hook());
        assert_eq!(snp_core::sync::lock(&out.0).as_slice(), b"first\nafter\n");
    }
}
