//! `tags_from_filename` / `tags_from_interpreter`: no panic on arbitrary
//! bytes, only the last path component matters, and the single-scan
//! basename lookup (U1) agrees with a straightforward reference built on the
//! public tables.
#![no_main]

use file_identify::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
use file_identify::{TagSet, tags_from_filename, tags_from_interpreter};
use libfuzzer_sys::fuzz_target;

/// identify's `tags_from_filename` for a name without separators, written
/// as the Python reads (`[filename] + filename.split('.')`, then
/// `os.path.splitext` and `ext.lower()`), without U1's shared scan and hash.
fn reference(name: &[u8]) -> TagSet {
    let mut tags = std::iter::once(name)
        .chain(name.split(|&b| b == b'.'))
        .find_map(|part| NAMES.get_bytes(part))
        .unwrap_or_default();
    let ext = name.iter().rposition(|&b| b == b'.').and_then(|dot| {
        let (stem, ext) = name.split_at(dot);
        (!stem.iter().all(|&b| b == b'.')).then(|| &ext[1..])
    });
    if let Some(ext) = ext {
        let lower = match std::str::from_utf8(ext) {
            Ok(s) => s.to_lowercase().into_bytes(),
            Err(_) => ext.to_vec(),
        };
        let found = EXTENSIONS
            .get_bytes(&lower)
            .or_else(|| EXTENSIONS_NEED_BINARY_CHECK.get_bytes(&lower));
        tags = tags.union(found.unwrap_or_default());
    }
    tags
}

fuzz_target!(|data: &[u8]| {
    let tags = tags_from_filename(data);
    // Directories never change the result (posixpath; on Windows a drive
    // prefix in `data` would not be a drive any more after "dir/").
    if cfg!(unix) {
        let mut nested = b"some/dir/".to_vec();
        nested.extend_from_slice(data);
        assert_eq!(tags_from_filename(&nested), tags);
        let name = data.rsplit(|&b| b == b'/').next().unwrap_or(data);
        assert_eq!(tags, reference(name), "{:?}", String::from_utf8_lossy(data));
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let interp = tags_from_interpreter(s);
        // Only the part after the last '/' is looked up.
        if let Some((_, last)) = s.rsplit_once('/') {
            assert_eq!(tags_from_interpreter(last), interp);
        }
    }
});
