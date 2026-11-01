//! lefthook's typed configuration (05 §3–§5; `internal/config/{hook,job,command,script}.go`), decoded from the
//! merged koanf tree with mapstructure's weak typing, and the job list of a hook (05 §6.1: jobs, then commands, then
//! scripts, each of the latter two sorted by `priority` and leading number).
//!
//! Owner: WP-34.

use super::value::{
    self, LMap, LValue, TypeError, field, weak_bool, weak_int, weak_str_list, weak_str_map,
    weak_string,
};

/// Where a job came from (the keys each kind accepts differ, 05 §5).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A `jobs:` entry.
    Job,
    /// A `commands:` entry.
    Command,
    /// A `scripts:` entry.
    Script,
}

/// A `group:` of a job.
#[derive(Debug, Clone)]
pub(crate) struct Group {
    /// `parallel` (wins over `piped` inside a group, `job.go:69-73`).
    pub(crate) parallel: bool,
    /// `piped`.
    pub(crate) piped: bool,
    /// Whether `jobs` was given at all.
    pub(crate) has_jobs: bool,
    /// The nested jobs.
    pub(crate) jobs: Vec<Job>,
}

/// One job (also a converted command or script).
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// The kind.
    pub(crate) origin: Origin,
    /// `name` (the map key for commands and scripts).
    pub(crate) name: String,
    /// `run`.
    pub(crate) run: String,
    /// `script` (the map key for scripts).
    pub(crate) script: String,
    /// `runner`.
    pub(crate) runner: String,
    /// `args`.
    pub(crate) args: String,
    /// `root`.
    pub(crate) root: String,
    /// `files` (a shell command).
    pub(crate) files: String,
    /// `fail_text`.
    pub(crate) fail_text: String,
    /// `timeout`: the Go duration text (an integer is nanoseconds); `None` when unset or zero.
    pub(crate) timeout: Option<String>,
    /// `glob`.
    pub(crate) glob: Vec<String>,
    /// `exclude`.
    pub(crate) exclude: Vec<String>,
    /// `tags`.
    pub(crate) tags: Vec<String>,
    /// `file_types`.
    pub(crate) file_types: Vec<String>,
    /// `env`.
    pub(crate) env: Vec<(String, String)>,
    /// `interactive`.
    pub(crate) interactive: bool,
    /// `use_stdin`.
    pub(crate) use_stdin: bool,
    /// `stage_fixed`.
    pub(crate) stage_fixed: bool,
    /// `skip` (raw condition).
    pub(crate) skip: Option<LValue>,
    /// `only` (raw condition).
    pub(crate) only: Option<LValue>,
    /// `group`.
    pub(crate) group: Option<Group>,
    /// `priority` (commands and scripts: ordering only).
    pub(crate) priority: i64,
}

/// One hook (`internal/config/hook.go`).
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    /// The hook name.
    pub(crate) name: String,
    /// `parallel`.
    pub(crate) parallel: bool,
    /// `piped`.
    pub(crate) piped: bool,
    /// `follow`.
    pub(crate) follow: bool,
    /// `fail_on_changes` (weak string; empty when unset).
    pub(crate) fail_on_changes: String,
    /// `files`.
    pub(crate) files: String,
    /// `exclude_tags`.
    pub(crate) exclude_tags: Vec<String>,
    /// `exclude`.
    pub(crate) exclude: Vec<String>,
    /// `skip`.
    pub(crate) skip: Option<LValue>,
    /// `only`.
    pub(crate) only: Option<LValue>,
    /// `setup[].run`.
    pub(crate) setup: Vec<String>,
    /// jobs ++ commands ++ scripts (05 §6.1).
    pub(crate) jobs: Vec<Job>,
}

/// The decoder of one map: `path` is the dotted key path for messages.
struct Dec<'a> {
    m: &'a LMap,
    path: &'a str,
}

impl Dec<'_> {
    fn key(&self, k: &str) -> String {
        if self.path.is_empty() {
            k.to_owned()
        } else {
            format!("{}.{k}", self.path)
        }
    }

    fn get(&self, k: &str) -> Option<&LValue> {
        field(self.m, k).filter(|v| !matches!(v, LValue::Null))
    }

    fn string(&self, k: &str) -> Result<String, TypeError> {
        self.get(k)
            .map_or_else(|| Ok(String::new()), |v| weak_string(v, &self.key(k)))
    }

    fn boolean(&self, k: &str) -> Result<bool, TypeError> {
        self.get(k)
            .map_or(Ok(false), |v| weak_bool(v, &self.key(k)))
    }

    fn list(&self, k: &str) -> Result<Vec<String>, TypeError> {
        self.get(k)
            .map_or_else(|| Ok(Vec::new()), |v| weak_str_list(v, &self.key(k)))
    }

    fn map(&self, k: &str) -> Result<Vec<(String, String)>, TypeError> {
        self.get(k)
            .map_or_else(|| Ok(Vec::new()), |v| weak_str_map(v, &self.key(k)))
    }

    fn int(&self, k: &str) -> Result<i64, TypeError> {
        self.get(k).map_or(Ok(0), |v| weak_int(v, &self.key(k)))
    }

    fn any(&self, k: &str) -> Option<LValue> {
        self.get(k).cloned()
    }

    /// `time.Duration` through `StringToTimeDuration`: a string is kept verbatim (checked later), a number is
    /// nanoseconds, zero means "no timeout".
    fn duration(&self, k: &str) -> Result<Option<String>, TypeError> {
        Ok(match self.get(k) {
            None => None,
            Some(LValue::Str(s)) => Some(s.clone()),
            Some(v @ (LValue::Int(_) | LValue::Float(_) | LValue::Bool(_))) => {
                let n = weak_int(v, &self.key(k))?;
                (n != 0).then(|| format!("{n}ns"))
            }
            Some(other) => return Err(TypeError::new(&self.key(k), "a duration", other)),
        })
    }
}

fn as_map<'a>(v: &'a LValue, path: &str) -> Result<&'a LMap, TypeError> {
    match v {
        LValue::Map(m) => Ok(m),
        other => Err(TypeError::new(path, "a map", other)),
    }
}

/// Decodes one `jobs:` entry (`path` = `pre-commit.jobs[0]`), groups recursively (bounded by the loader's depth).
fn job(v: &LValue, path: &str, depth: usize) -> Result<Job, TypeError> {
    let m = as_map(v, path)?;
    let d = Dec { m, path };
    let group = match d.get("group") {
        None => None,
        Some(g) => {
            let gpath = d.key("group");
            let gm = as_map(g, &gpath)?;
            let gd = Dec {
                m: gm,
                path: &gpath,
            };
            let mut jobs = Vec::new();
            let has_jobs = gd.get("jobs").is_some();
            if let Some(js) = gd.get("jobs") {
                let LValue::List(items) = js else {
                    return Err(TypeError::new(&gd.key("jobs"), "a list", js));
                };
                if depth > value::MAX_DEPTH {
                    return Err(TypeError {
                        path: gpath,
                        msg: "groups are nested too deeply".to_owned(),
                    });
                }
                for (i, item) in items.iter().enumerate() {
                    jobs.push(job(
                        item,
                        &format!("{gpath}.jobs[{i}]"),
                        depth.saturating_add(1),
                    )?);
                }
            }
            Some(Group {
                parallel: gd.boolean("parallel")?,
                piped: gd.boolean("piped")?,
                has_jobs,
                jobs,
            })
        }
    };
    Ok(Job {
        origin: Origin::Job,
        name: d.string("name")?,
        run: d.string("run")?,
        script: d.string("script")?,
        runner: d.string("runner")?,
        args: d.string("args")?,
        root: d.string("root")?,
        files: d.string("files")?,
        fail_text: d.string("fail_text")?,
        timeout: d.duration("timeout")?,
        glob: d.list("glob")?,
        exclude: d.list("exclude")?,
        tags: d.list("tags")?,
        file_types: d.list("file_types")?,
        env: d.map("env")?,
        interactive: d.boolean("interactive")?,
        use_stdin: d.boolean("use_stdin")?,
        stage_fixed: d.boolean("stage_fixed")?,
        skip: d.any("skip"),
        only: d.any("only"),
        group,
        priority: 0,
    })
}

/// A command (`command.go`) as a job.
fn command(name: &str, v: &LValue, path: &str) -> Result<Job, TypeError> {
    let m = as_map(v, path)?;
    let d = Dec { m, path };
    Ok(Job {
        origin: Origin::Command,
        name: name.to_owned(),
        run: d.string("run")?,
        script: String::new(),
        runner: String::new(),
        args: String::new(),
        root: d.string("root")?,
        files: d.string("files")?,
        fail_text: d.string("fail_text")?,
        timeout: d.duration("timeout")?,
        glob: d.list("glob")?,
        exclude: d.list("exclude")?,
        tags: d.list("tags")?,
        file_types: d.list("file_types")?,
        env: d.map("env")?,
        interactive: d.boolean("interactive")?,
        use_stdin: d.boolean("use_stdin")?,
        stage_fixed: d.boolean("stage_fixed")?,
        skip: d.any("skip"),
        only: d.any("only"),
        group: None,
        priority: d.int("priority")?,
    })
}

/// A script (`script.go`) as a job.
fn script(name: &str, v: &LValue, path: &str) -> Result<Job, TypeError> {
    let m = as_map(v, path)?;
    let d = Dec { m, path };
    Ok(Job {
        origin: Origin::Script,
        name: name.to_owned(),
        run: String::new(),
        script: name.to_owned(),
        runner: d.string("runner")?,
        args: d.string("args")?,
        root: String::new(),
        files: String::new(),
        fail_text: d.string("fail_text")?,
        timeout: d.duration("timeout")?,
        glob: Vec::new(),
        exclude: Vec::new(),
        tags: d.list("tags")?,
        file_types: Vec::new(),
        env: d.map("env")?,
        interactive: d.boolean("interactive")?,
        use_stdin: d.boolean("use_stdin")?,
        stage_fixed: d.boolean("stage_fixed")?,
        skip: d.any("skip"),
        only: d.any("only"),
        group: None,
        priority: d.int("priority")?,
    })
}

/// `parseNum` (`script.go:87-106`): the leading decimal number of a name, `-1` without one (or on overflow).
fn parse_num(s: &str) -> i64 {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        -1
    } else {
        digits.parse().unwrap_or(-1)
    }
}

/// The comparator of `CommandsToJobs` / `ScriptsToJobs` (`command.go:59-92`).
fn by_priority(a: &Job, b: &Job) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.priority != 0 || b.priority != 0 {
        if a.priority == 0 {
            return Ordering::Greater;
        }
        if b.priority == 0 {
            return Ordering::Less;
        }
        return a.priority.cmp(&b.priority);
    }
    let (x, y) = (parse_num(&a.name), parse_num(&b.name));
    match (x, y) {
        (-1, -1) => a.name.as_bytes().cmp(b.name.as_bytes()),
        (-1, _) => Ordering::Greater,
        (_, -1) => Ordering::Less,
        _ => x.cmp(&y),
    }
}

/// Decodes one hook map.
pub(crate) fn hook(name: &str, m: &LMap) -> Result<Hook, TypeError> {
    let d = Dec { m, path: name };
    let mut jobs = Vec::new();
    if let Some(js) = d.get("jobs") {
        let LValue::List(items) = js else {
            return Err(TypeError::new(&d.key("jobs"), "a list", js));
        };
        for (i, item) in items.iter().enumerate() {
            jobs.push(job(item, &format!("{name}.jobs[{i}]"), 0)?);
        }
    }
    let named = |key: &str,
                 f: fn(&str, &LValue, &str) -> Result<Job, TypeError>|
     -> Result<Vec<Job>, TypeError> {
        let mut out = Vec::new();
        match d.get(key) {
            None => {}
            Some(LValue::Map(cm)) => {
                for (k, v) in cm {
                    if matches!(v, LValue::Null) {
                        continue;
                    }
                    out.push(f(k, v, &format!("{name}.{key}.{k}"))?);
                }
            }
            Some(other) => return Err(TypeError::new(&d.key(key), "a map", other)),
        }
        out.sort_by(by_priority);
        Ok(out)
    };
    jobs.extend(named("commands", command)?);
    jobs.extend(named("scripts", script)?);
    let mut setup = Vec::new();
    if let Some(s) = d.get("setup") {
        let LValue::List(items) = s else {
            return Err(TypeError::new(&d.key("setup"), "a list", s));
        };
        for (i, item) in items.iter().enumerate() {
            let p = format!("{name}.setup[{i}]");
            let sm = as_map(item, &p)?;
            setup.push(Dec { m: sm, path: &p }.string("run")?);
        }
    }
    Ok(Hook {
        name: name.to_owned(),
        parallel: d.boolean("parallel")?,
        piped: d.boolean("piped")?,
        follow: d.boolean("follow")?,
        fail_on_changes: d.string("fail_on_changes")?,
        files: d.string("files")?,
        exclude_tags: d.list("exclude_tags")?,
        exclude: d.list("exclude")?,
        skip: d.any("skip"),
        only: d.any("only"),
        setup,
        jobs,
    })
}

/// The printable name of a job (`job.go:40-52`): `name`, else `run`, else `script`, else `[<index>]`.
pub(crate) fn printable_name(j: &Job, index: usize) -> String {
    if !j.name.is_empty() {
        j.name.clone()
    } else if !j.run.is_empty() {
        j.run.clone()
    } else if !j.script.is_empty() {
        j.script.clone()
    } else {
        format!("[{index}]")
    }
}

#[cfg(test)]
mod tests {
    use super::super::value::{Format, parse};
    use super::*;

    fn hook_of(src: &str) -> Hook {
        let m = parse(src, Format::Yaml).unwrap();
        hook("pre-commit", m["pre-commit"].as_map().unwrap()).unwrap()
    }

    #[test]
    fn job_list_order() {
        let h = hook_of(
            "pre-commit:\n  jobs:\n    - run: j1\n  commands:\n    b: {run: b}\n    a: {run: a}\n    2_x: {run: x}\n    10_y: {run: y}\n    p: {run: p, priority: 1}\n  scripts:\n    s.sh: {runner: bash}\n",
        );
        let names: Vec<&str> = h.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["", "p", "2_x", "10_y", "a", "b", "s.sh"]);
        assert_eq!(h.jobs[6].script, "s.sh");
        assert_eq!(h.jobs[6].origin, Origin::Script);
    }

    #[test]
    fn weak_fields() {
        let h = hook_of(
            "pre-commit:\n  parallel: 'true'\n  jobs:\n    - name: x\n      glob: 3\n      tags: a b\n      timeout: 15\n      env: [{A: 1}, {B: true}]\n      group: {jobs: [{run: y}]}\n",
        );
        assert!(h.parallel);
        let j = &h.jobs[0];
        assert_eq!(j.glob, ["3"]);
        assert_eq!(j.tags, ["a b"]);
        assert_eq!(j.timeout.as_deref(), Some("15ns"));
        assert_eq!(
            j.env,
            [
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "1".to_owned())
            ]
        );
        assert_eq!(j.group.as_ref().unwrap().jobs[0].run, "y");
    }

    #[test]
    fn type_errors_name_the_key() {
        let m = parse("pre-commit:\n  jobs:\n    - run: [a]\n", Format::Yaml).unwrap();
        let e = hook("pre-commit", m["pre-commit"].as_map().unwrap()).unwrap_err();
        assert_eq!(e.path, "pre-commit.jobs[0].run");
        assert_eq!(e.msg, "expected a string, got a list");
    }
}
