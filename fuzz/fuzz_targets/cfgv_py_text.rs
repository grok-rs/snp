//! CPython text helpers never panic on arbitrary input.
#![no_main]

use cfgv::{RegexPattern, Value, py};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match py::decode_utf8(data) {
        Ok(text) => {
            let repr = py::repr_str(text);
            assert!(repr.starts_with(['\'', '"']));
            let _ = py::universal_newlines(text);
            assert_eq!(Value::from(text).repr(), repr);
        }
        Err(err) => {
            assert!(err.start() < err.end() && err.end() <= data.len());
            let _ = err.to_string();
        }
    }
    let bytes = py::repr_bytes(data);
    assert!(bytes.starts_with('b'));
    assert_eq!(
        RegexPattern::Bytes(data).to_text().chars().count(),
        data.len()
    );
    if let Some(chunk) = data.get(..8) {
        let bits = u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8]));
        let _ = py::repr_float(f64::from_bits(bits));
    }
});
