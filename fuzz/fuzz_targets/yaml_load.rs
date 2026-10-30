//! Fuzz target `yaml_load`: PyYAML `SafeLoader` load (`decode_text` + `load`), and the dumper on what loads.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Beyond "no panic": `redump` succeeds exactly when `load` does, and the dump of a loaded value loads back to a
//! value with the same dump (`dump(load(dump(v))) == dump(v)`; tuples come back as lists, which dump alike).
//!
//! Owner: WP-14.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_yaml::Limits;
use snp_yaml::emit::{dump, redump};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = snp_yaml::decode_text(data) else {
        return;
    };
    let lim = Limits::default();
    let loaded = snp_yaml::load(&text, &lim);
    let redumped = redump(&text, &lim);
    assert_eq!(
        loaded.is_ok(),
        redumped.is_ok(),
        "load and redump disagree on {text:?}"
    );
    if let Ok(v) = loaded {
        let out = dump(&v);
        match snp_yaml::load(&out, &lim) {
            Ok(back) => assert_eq!(dump(&back), out, "round trip of {text:?}"),
            Err(e) => panic!("the dump of {text:?} does not load: {out:?}: {e}"),
        }
    }
});
