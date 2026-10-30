//! Fuzz target `lang_docker_mountinfo`: the docker container-detection parsers of `language: docker` /
//! `docker_image` — `/proc/1/mountinfo` (`_get_container_id`), `docker inspect` output (`container['Mounts']`) and
//! `docker system info` output (`_is_rootless`) — over arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks; a container id, when
//! found, is 64 lowercase hex/digit characters taken from the input.
//!
//! Owner: WP-25.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_lang::langs::docker::{container_id_from_mountinfo, mounts_from_inspect, rootless_from_info};

fuzz_target!(|data: &[u8]| {
    if let Some(id) = container_id_from_mountinfo(data) {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            panic!("malformed container id {id:?}");
        }
        if !data.windows(64).any(|w| w == id.as_bytes()) {
            panic!("container id not taken from the input");
        }
    }
    let _mounts = mounts_from_inspect(data);
    let _rootless = rootless_from_info(data);
});
