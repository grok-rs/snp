//! Fuzz target `lang_docker_mountinfo`: the input parsers of the group-2 languages over arbitrary bytes — the docker
//! container-detection parsers of `language: docker` / `docker_image` (`/proc/1/mountinfo` for `_get_container_id`,
//! `docker inspect` output for `container['Mounts']`, `docker system info` output for `_is_rootless`), the dart
//! `pubspec.yaml` reader and the dotnet `.nuspec` tool-id reader (both on the input as UTF-8, when it is).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks; a container id, when
//! found, is 64 characters of `[a-z0-9]` (pre-commit's `_HOSTNAME_MOUNT_RE` group, which is not limited to hex)
//! taken from the input; a nuspec tool id is non-empty.
//!
//! Owner: WP-25.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_lang::langs::dart::pubspec_executables;
use snp_lang::langs::docker::{
    container_id_from_mountinfo, mounts_from_inspect, rootless_from_info,
};
use snp_lang::langs::dotnet::nuspec_tool_id;

fuzz_target!(|data: &[u8]| {
    if let Some(id) = container_id_from_mountinfo(data) {
        if id.len() != 64 || !id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z')) {
            panic!("malformed container id {id:?}");
        }
        if !data.windows(64).any(|w| w == id.as_bytes()) {
            panic!("container id not taken from the input");
        }
    }
    let _mounts = mounts_from_inspect(data);
    let _rootless = rootless_from_info(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let _executables = pubspec_executables(text);
        if let Ok(id) = nuspec_tool_id(text) {
            if id.is_empty() {
                panic!("empty nuspec tool id");
            }
        }
    }
});
