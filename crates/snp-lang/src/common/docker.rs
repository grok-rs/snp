//! The shared `docker run` prefix of `language: docker` and `language: docker_image` (`docker.py:23-66,109-156`).
//!
//! `docker_cmd(color)` = `docker run --rm [--tty] [-u uid:gid] -v <host path of the cwd>:/src:rw,Z --workdir /src`:
//!
//! * `--tty` iff colour (the hook runs on a pty);
//! * `-u {getuid}:{getgid}` unless the daemon is rootless (`docker system info --format '{{ json . }}'`, memoised per
//!   process in the `ToolProbe`; a failing probe or unparsable output means "not rootless", D-CRASH-21 — pre-commit
//!   raises `CalledProcessError` there) and never on Windows (no `os.getuid`);
//! * the cwd mapped to the host path when snp itself runs in a container that talks to the host daemon: the
//!   container id comes from `/proc/1/mountinfo` (`/containers[/overlay-containers]/<64 hex>[/userdata]/hostname`),
//!   its bind mounts from `docker inspect <id>` (memoised); the first mount whose destination contains the cwd
//!   rewrites it with Python's `str.replace` (parity); any failure (no container, inspect failing, unexpected JSON)
//!   leaves the path unchanged.
//!
//! Owner: WP-16 (shared with WP-25's `language: docker`).

use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde_json::Value;

use crate::{Host, LangError};

/// The probe of `_is_rootless` (`docker.py:111-113`), argv verbatim.
pub(crate) const ROOTLESS_PROBE: [&str; 5] =
    ["docker", "system", "info", "--format", "{{ json . }}"];

/// `docker_cmd(color=color)` (`docker.py:145-156`).
pub(crate) fn docker_cmd(host: &Host<'_>, color: bool) -> Result<Vec<OsString>, LangError> {
    let mut cmd: Vec<OsString> = ["docker", "run", "--rm"].map(OsString::from).into();
    if color {
        cmd.push("--tty".into());
    }
    cmd.extend(docker_user(host)?);
    let mut volume = docker_path(host, host.cwd.as_os_str())?;
    volume.push(":/src:rw,Z");
    cmd.push("-v".into());
    cmd.push(volume);
    cmd.push("--workdir".into());
    cmd.push("/src".into());
    Ok(cmd)
}

/// `get_docker_user()` (`docker.py:131-138`): the rootless probe runs first, on every OS.
fn docker_user(host: &Host<'_>) -> Result<Vec<OsString>, LangError> {
    if is_rootless(host)? {
        return Ok(Vec::new());
    }
    #[cfg(unix)]
    {
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        Ok(vec!["-u".into(), format!("{uid}:{gid}").into()])
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
    }
}

/// `_is_rootless()` (`docker.py:109-128`), memoised in the host's `ToolProbe`.
pub(crate) fn is_rootless(host: &Host<'_>) -> Result<bool, LangError> {
    if let Some(v) = host.probe.docker.rootless.get() {
        return Ok(*v);
    }
    let argv: Vec<OsString> = ROOTLESS_PROBE.iter().map(OsString::from).collect();
    let out = host.runner.run(&argv, host.base_env, host.cwd)?;
    let rootless = out.status.0 == 0 && rootless_from_info(&out.stdout);
    Ok(*host.probe.docker.rootless.get_or_init(|| rootless))
}

/// `'name=rootless' in (info.get('SecurityOptions') or ()) or info['host']['security']['rootless']`, where anything
/// that would raise in Python (invalid JSON, a non-object, a missing key) is false.
pub(crate) fn rootless_from_info(stdout: &[u8]) -> bool {
    let Ok(Value::Object(info)) = serde_json::from_slice::<Value>(stdout) else {
        return false;
    };
    let in_security_options = match info.get("SecurityOptions") {
        Some(Value::Array(opts)) => opts.iter().any(|o| o.as_str() == Some("name=rootless")),
        Some(Value::String(s)) => s.contains("name=rootless"),
        Some(Value::Object(m)) => m.contains_key("name=rootless"),
        _ => false,
    };
    in_security_options
        || info
            .get("host")
            .and_then(|h| h.get("security"))
            .and_then(|s| s.get("rootless"))
            .is_some_and(truthy)
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `_get_docker_path(path)` (`docker.py:45-66`).
fn docker_path(host: &Host<'_>, path: &OsStr) -> Result<OsString, LangError> {
    let mounts = if let Some(m) = host.probe.docker.mounts.get() {
        m.clone()
    } else {
        let m = container_mounts(host)?;
        host.probe.docker.mounts.get_or_init(|| m).clone()
    };
    let Some(mounts) = mounts else {
        return Ok(path.to_os_string());
    };
    for (dest, src) in &mounts {
        let common = snp_pycompat::ospath::commonpath(&[path, dest.as_os_str()]);
        if common.is_ok_and(|c| c == *dest) {
            return Ok(py_replace(path, dest, src));
        }
    }
    Ok(path.to_os_string())
}

/// The bind mounts `(Destination, Source)` of the container snp runs in; `None` outside a container or when docker
/// cannot tell.
fn container_mounts(host: &Host<'_>) -> Result<Option<Vec<(OsString, OsString)>>, LangError> {
    let mountinfo = host
        .probe
        .docker
        .mountinfo
        .as_deref()
        .unwrap_or_else(|| Path::new("/proc/1/mountinfo"));
    let Ok(bytes) = std::fs::read(mountinfo) else {
        return Ok(None);
    };
    let Some(id) = container_id(&bytes) else {
        return Ok(None);
    };
    let argv: Vec<OsString> = vec!["docker".into(), "inspect".into(), id.into()];
    let out = host.runner.run(&argv, host.base_env, host.cwd)?;
    if out.status.0 != 0 {
        // Docker-in-docker: our own container is not visible from this daemon.
        return Ok(None);
    }
    Ok(mounts_from_inspect(&out.stdout))
}

/// `container, = json.loads(out)` then `container['Mounts']` as `(Destination, Source)` pairs.
pub(crate) fn mounts_from_inspect(stdout: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    let Ok(Value::Array(containers)) = serde_json::from_slice::<Value>(stdout) else {
        return None;
    };
    let [container] = containers.as_slice() else {
        return None;
    };
    let mounts = container.get("Mounts")?.as_array()?;
    mounts
        .iter()
        .map(|m| {
            let src = m.get("Source")?.as_str()?;
            let dest = m.get("Destination")?.as_str()?;
            Some((OsString::from(dest), OsString::from(src)))
        })
        .collect()
}

/// `_get_container_id()` (`docker.py:34-42`): group 1 of the first match of
/// `rb'/containers(?:/overlay-containers)?/([a-z0-9]{64})(?:/userdata)?/hostname'` in the lines of `mountinfo`.
pub(crate) fn container_id(mountinfo: &[u8]) -> Option<String> {
    const PREFIX: &[u8] = b"/containers";
    for line in mountinfo.split_inclusive(|b| *b == b'\n') {
        let mut from = 0_usize;
        while let Some(rest) = line.get(from..) {
            let Some(pos) = find(rest, PREFIX) else { break };
            let start = from.saturating_add(pos);
            if let Some(id) = line
                .get(start.saturating_add(PREFIX.len())..)
                .and_then(match_after_containers)
            {
                return Some(id);
            }
            from = start.saturating_add(1);
        }
    }
    None
}

/// The rest of the regex after `/containers`: `(?:/overlay-containers)?/([a-z0-9]{64})(?:/userdata)?/hostname`.
fn match_after_containers(s: &[u8]) -> Option<String> {
    let try_id = |s: &[u8]| -> Option<String> {
        let rest = s.strip_prefix(b"/")?;
        let (id, tail) = rest.split_at_checked(64)?;
        if !id
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return None;
        }
        let ok = tail.starts_with(b"/hostname") || tail.starts_with(b"/userdata/hostname");
        ok.then(|| String::from_utf8_lossy(id).into_owned())
    };
    s.strip_prefix(b"/overlay-containers")
        .and_then(try_id)
        .or_else(|| try_id(s))
}

/// The first index of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    (0..hay.len()).find(|&i| hay.get(i..).is_some_and(|t| t.starts_with(needle)))
}

/// Python `str.replace(old, new)` over OS strings (every non-overlapping occurrence, left to right).
fn py_replace(s: &OsStr, old: &OsStr, new: &OsStr) -> OsString {
    let (s, old, new) = (
        s.as_encoded_bytes(),
        old.as_encoded_bytes(),
        new.as_encoded_bytes(),
    );
    if old.is_empty() {
        return os(s);
    }
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0_usize;
    while let Some(rest) = s.get(i..) {
        if rest.is_empty() {
            break;
        }
        if rest.starts_with(old) {
            out.extend_from_slice(new);
            i = i.saturating_add(old.len());
        } else {
            out.extend_from_slice(rest.get(..1).unwrap_or_default());
            i = i.saturating_add(1);
        }
    }
    os(&out)
}

/// Bytes produced from valid OS-string encodings (concatenations of them) back to an `OsString`.
fn os(b: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(b).to_os_string()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tests/languages/docker_test.py:17-215` mountinfo samples (abridged to the relevant lines).
    const DOCKER_CGROUP_V1: &[u8] = b"759 717 0:52 / / rw,relatime master:300 - overlay overlay rw,lowerdir=/var/lib/docker/overlay2/l/PCPE5P5IVGM7CFCPJR353N3ONK\n\
761 759 0:58 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw\n\
766 759 8:5 /var/lib/docker/containers/c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7/resolv.conf /etc/resolv.conf rw,relatime - ext4 /dev/sda5 rw,errors=remount-ro\n\
767 759 8:5 /var/lib/docker/containers/c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7/hostname /etc/hostname rw,relatime - ext4 /dev/sda5 rw,errors=remount-ro\n";
    const PODMAN: &[u8] = b"1200 915 0:57 / / rw,relatime - overlay overlay rw\n\
1213 1200 0:55 /containers/overlay-containers/c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7/userdata/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw\n";
    const ID: &str = "c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7";

    #[test]
    fn container_ids() {
        assert_eq!(container_id(DOCKER_CGROUP_V1).as_deref(), Some(ID));
        assert_eq!(container_id(PODMAN).as_deref(), Some(ID));
        assert_eq!(container_id(b"1 2 0:1 / / rw - ext4 /dev/sda1 rw\n"), None);
        assert_eq!(container_id(b""), None);
        // Uppercase hex and short ids do not match the regex.
        let upper = format!(
            "/var/lib/docker/containers/{}/hostname\n",
            ID.to_ascii_uppercase()
        );
        assert_eq!(container_id(upper.as_bytes()), None);
        assert_eq!(container_id(b"/containers/abc/hostname\n"), None);
        // A later occurrence on the same line still matches (re.search).
        let two = format!("/containers/x /containers/{ID}/hostname\n");
        assert_eq!(container_id(two.as_bytes()).as_deref(), Some(ID));
    }

    #[test]
    fn rootless_detection() {
        assert!(rootless_from_info(
            br#"{"SecurityOptions": ["name=seccomp,profile=default", "name=rootless"]}"#
        ));
        assert!(rootless_from_info(
            br#"{"host": {"security": {"rootless": true}}}"#
        ));
        assert!(!rootless_from_info(br#"{"SecurityOptions": null}"#));
        assert!(!rootless_from_info(
            br#"{"SecurityOptions": ["name=seccomp"]}"#
        ));
        assert!(!rootless_from_info(
            br#"{"host": {"security": {"rootless": false}}}"#
        ));
        assert!(!rootless_from_info(b"not json"));
        assert!(!rootless_from_info(b"[1]"));
    }

    #[test]
    fn inspect_mounts() {
        let one = br#"[{"Mounts": [{"Source": "/opt/my_code", "Destination": "/project"}]}]"#;
        assert_eq!(
            mounts_from_inspect(one),
            Some(vec![(
                OsString::from("/project"),
                OsString::from("/opt/my_code")
            )])
        );
        assert_eq!(mounts_from_inspect(b"[]"), None);
        assert_eq!(mounts_from_inspect(b"[{}, {}]"), None);
        assert_eq!(mounts_from_inspect(b"{}"), None);
        assert_eq!(mounts_from_inspect(b"garbage"), None);
    }

    #[test]
    fn python_replace() {
        let r = |s: &str, a: &str, b: &str| {
            py_replace(OsStr::new(s), OsStr::new(a), OsStr::new(b))
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(
            r("/project/test/something", "/project", "/opt/my_code"),
            "/opt/my_code/test/something"
        );
        assert_eq!(r("/a/b/a/c", "/a", "/x"), "/x/b/x/c");
        assert_eq!(r("aaa", "aa", "b"), "ba");
    }
}
