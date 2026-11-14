//! `tags_from_filename` / `tags_from_interpreter`: no panic on arbitrary
//! bytes, and only the last path component matters.
#![no_main]

use file_identify::{tags_from_filename, tags_from_interpreter};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let tags = tags_from_filename(data);
    // Directories never change the result (posixpath; on Windows a drive
    // prefix in `data` would not be a drive any more after "dir/").
    if cfg!(unix) {
        let mut nested = b"some/dir/".to_vec();
        nested.extend_from_slice(data);
        assert_eq!(tags_from_filename(&nested), tags);
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let interp = tags_from_interpreter(s);
        // Only the part after the last '/' is looked up.
        if let Some((_, last)) = s.rsplit_once('/') {
            assert_eq!(tags_from_interpreter(last), interp);
        }
    }
});
