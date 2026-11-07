//! Launching a main class with the pinned JDK's `java`.
//!
//! The [`Runner`] trait is the extension point; `PlainMainRunner` spawns the
//! JVM in the foreground and forwards signals, output and the exit code.
//! When the manifest does not pin `[run].main`, `discover_main` finds
//! candidate classes in `target/classes` by parsing class files directly —
//! no JVM gets started for that. (The launcher rule jx matches: a static
//! `void main(String[])` method; instance-main forms are a newer, post-V1
//! discovery gap recorded in docs/KNOWN_LIMITATIONS.md.)

use std::path::{Path, PathBuf};
use std::process::Command;

use jx_common::error::{JxError, Kind};

/// What a runner gets, plus room to grow (classpath, debug, env).
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub java: PathBuf,
    pub classpath: Vec<String>,
    pub main_class: String,
    pub argv: Vec<String>,
    pub debug_port: Option<u16>,
}

/// The runnable extension point. Future run adapters (Spring Boot and
/// friends, post-V1) implement this with different classpath/entry logic.
pub trait Runner {
    fn run(&self, spec: &RunSpec) -> Result<i32, JxError>;
}

/// Foreground `java` invocation; the child inherits stdio so Ctrl+C behaves
/// and diagnostics need no re-routing.
pub struct PlainMainRunner;

impl Runner for PlainMainRunner {
    fn run(&self, spec: &RunSpec) -> Result<i32, JxError> {
        let mut cmd = Command::new(&spec.java);
        cmd.arg("-cp")
            .arg(spec.classpath.join(classpath_separator()));
        if let Some(port) = spec.debug_port {
            cmd.arg(format!(
                "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=*:{port}"
            ));
        }
        cmd.arg(&spec.main_class);
        cmd.args(&spec.argv);
        let status = cmd.status().map_err(|e| {
            JxError::new(Kind::Build, "cannot launch java from the pinned JDK")
                .at(spec.java.display().to_string())
                .caused_by(e)
                .hint("run `jx doctor`; the JDK install may be damaged")
        })?;
        if let Some(code) = status.code() {
            return Ok(code);
        }
        Err(JxError::new(Kind::Build, "the JVM was terminated by a signal").hint(
            "if you interrupted it on purpose, this is normal; otherwise include the message above in a bug report",
        ))
    }
}

/// Parse a class file just far enough to tell whether it declares a static
/// method named `main` taking `String[]` and returning void.
pub fn class_has_static_main(bytes: &[u8]) -> bool {
    let mut r = Reader {
        data: bytes,
        pos: 0,
    };
    if !r.skip(4) {
        return false; // magic
    }
    if !r.skip(2 + 2) {
        return false; // minor, major versions
    }
    let cp_count = match r.u16be() {
        Some(n) => n,
        None => return false,
    };
    let mut utf8: Vec<Option<String>> = vec![None; cp_count as usize];
    let mut i = 1;
    while i < cp_count {
        let tag = match r.u8() {
            Some(t) => t,
            None => return false,
        };
        match tag {
            1 => match r.u16be() {
                Some(len) => {
                    let mut buf = vec![0u8; len as usize];
                    if !r.read(&mut buf) {
                        return false;
                    }
                    utf8[i as usize] = String::from_utf8_lossy(&buf).into_owned().into();
                }
                None => return false,
            },
            3 | 4 => {
                if !r.skip(4) {
                    return false;
                }
            }
            5 | 6 => {
                // long/double take two constant pool slots
                if !r.skip(8) {
                    return false;
                }
                i += 1;
                if i >= cp_count {
                    break;
                }
            }
            7 | 8 | 16 | 19 | 20 => {
                if !r.skip(2) {
                    return false;
                }
            }
            9 | 10 | 11 | 12 | 17 | 18 => {
                if !r.skip(4) {
                    return false;
                }
            }
            15 => {
                if !r.skip(3) {
                    return false;
                }
            }
            _ => return false,
        }
        i += 1;
    }
    let _class_flags = match r.u16be() {
        Some(v) => v,
        None => return false,
    };
    if !r.skip(2 + 2) {
        return false; // this, super
    }
    let iface_count = match r.u16be() {
        Some(n) => n,
        None => return false,
    };
    if !r.skip(2 * iface_count as usize) {
        return false;
    }
    let field_count = match r.u16be() {
        Some(n) => n,
        None => return false,
    };
    if scan_members(&mut r, field_count, &utf8, false).is_err() {
        return false;
    }
    let method_count = match r.u16be() {
        Some(n) => n,
        None => return false,
    };
    scan_members(&mut r, method_count, &utf8, true).unwrap_or(false)
}

/// Walk member entries; `Ok(found_main)` or `Err(())` when truncated.
fn scan_members(
    r: &mut Reader,
    count: u16,
    utf8: &[Option<String>],
    want_main: bool,
) -> Result<bool, ()> {
    let mut found = false;
    for _ in 0..count {
        let flags = r.u16be().ok_or(())?;
        let name = r.u16be().ok_or(())?;
        let desc = r.u16be().ok_or(())?;
        let attrs = r.u16be().ok_or(())?;
        for _ in 0..attrs {
            r.skip(2).ok_or(())?;
            let len = r.u32be().ok_or(())? as usize;
            r.skip(len).ok_or(())?;
        }
        if want_main
            && flags & 0x0008 != 0 // ACC_STATIC
            && utf8.get(name as usize).and_then(|o| o.as_deref()) == Some("main")
            && utf8.get(desc as usize).and_then(|o| o.as_deref()) == Some("([Ljava/lang/String;)V")
        {
            found = true;
        }
    }
    Ok(found)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = self.data.get(self.pos).copied()?;
        self.pos += 1;
        Some(v)
    }
    fn u16be(&mut self) -> Option<u16> {
        let hi = self.u8()? as u16;
        let lo = self.u8()? as u16;
        Some((hi << 8) | lo)
    }
    fn u32be(&mut self) -> Option<u32> {
        let hi = self.u16be()? as u32;
        let lo = self.u16be()? as u32;
        Some((hi << 16) | lo)
    }
    fn skip(&mut self, n: usize) -> bool {
        match self.pos.checked_add(n) {
            Some(next) if next <= self.data.len() => {
                self.pos = next;
                true
            }
            _ => false,
        }
    }
    fn read(&mut self, out: &mut [u8]) -> bool {
        let Some(next) = self.pos.checked_add(out.len()) else {
            return false;
        };
        if next > self.data.len() {
            return false;
        }
        out.copy_from_slice(&self.data[self.pos..self.pos + out.len()]);
        self.pos += out.len();
        true
    }
}

/// Classes with an entrypoint in `classes_dir`, fully qualified by directory
/// position (package `app` lives under `app/`). Deterministic order.
pub fn discover_main(classes_dir: &Path) -> Result<Vec<String>, JxError> {
    if !classes_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let mut stack = vec![classes_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| {
                JxError::new(Kind::Io, "cannot list compiled classes")
                    .at(dir.display().to_string())
                    .caused_by(e)
            })?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "class") {
                let bytes = std::fs::read(&path).map_err(|e| {
                    JxError::new(Kind::Io, "cannot read a compiled class")
                        .at(path.display().to_string())
                        .caused_by(e)
                })?;
                if class_has_static_main(&bytes) {
                    let relative = path
                        .strip_prefix(classes_dir)
                        .expect("walk stays inside classes_dir");
                    let fqn = relative
                        .with_extension("")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(".");
                    found.push(fqn);
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The `target/classes` file for a fully qualified class name, when the name
/// maps to a path below `classes_dir`. Names that escape the directory do
/// not map, so a bad `[run] main` fails before the JVM reports it.
pub fn class_file_path(classes_dir: &Path, class_name: &str) -> Option<PathBuf> {
    if class_name.is_empty() {
        return None;
    }
    let mut path = classes_dir.to_path_buf();
    for segment in class_name.split('.') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('/')
            || segment.contains('\\')
            || segment.contains('\0')
        {
            return None;
        }
        path.push(segment);
    }
    path.set_extension("class");
    if path.starts_with(classes_dir) {
        Some(path)
    } else {
        None
    }
}

/// Whether `[run] main` names a class file the last build actually produced.
pub fn compiled_class_exists(classes_dir: &Path, class_name: &str) -> bool {
    class_file_path(classes_dir, class_name).is_some_and(|path| path.is_file())
}

/// Names the main class for a run: pinned in the manifest, else discovered.
/// One candidate wins; several fail loudly listing them (principle 6).
pub fn decide_main(classes_dir: &Path, pinned: Option<&String>) -> Result<String, JxError> {
    if let Some(pinned) = pinned {
        return Ok(pinned.clone());
    }
    let candidates = discover_main(classes_dir)?;
    match candidates.len() {
        0 => Err(
            JxError::new(Kind::Build, "no main class found")
                .at(classes_dir.display().to_string())
                .hint(
                    "compile first with `jx build`; or pin `[run] main = \"com.pkg.Main\"` in project.toml",
                ),
        ),
        1 => Ok(candidates.into_iter().next().expect("len 1 checked")),
        _ => {
            let mut msg = "several classes define a static main:".to_string();
            for c in &candidates {
                msg.push_str("\n        ");
                msg.push_str(c);
            }
            Err(
                JxError::new(Kind::Build, msg)
                    .hint("pin `[run] main = \"com.pkg.Main\"` in project.toml to choose one"),
            )
        }
    }
}

/// `;` on Windows, `:` elsewhere.
pub fn classpath_separator() -> &'static str {
    if std::env::consts::OS == "windows" {
        ";"
    } else {
        ":"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Build a minimal class file blob with utf8 entries and one method.
    fn class_bytes(utf8s: &[&str], method_flags: u16, name_idx: u16, desc_idx: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(0xCAFEBABEu32.to_be_bytes());
        out.extend(0u16.to_be_bytes()); // minor
        out.extend(52u16.to_be_bytes()); // major: Java 8+
        out.extend((utf8s.len() as u16 + 1).to_be_bytes()); // pool count
        for s in utf8s {
            out.push(1);
            out.extend((s.len() as u16).to_be_bytes());
            out.extend(s.as_bytes());
        }
        out.extend(0x0021u16.to_be_bytes()); // class flags: public super
        out.extend(1u16.to_be_bytes()); // this class
        out.extend(0u16.to_be_bytes()); // super
        out.extend(0u16.to_be_bytes()); // interfaces
        out.extend(1u16.to_be_bytes()); // fields (one, with descriptors below)
        // field: flags/name/desc/attrs referencing the utf8 entries we wrote
        out.extend(0x0002u16.to_be_bytes());
        out.extend(3u16.to_be_bytes());
        out.extend(3u16.to_be_bytes());
        out.extend(0u16.to_be_bytes());
        out.extend(1u16.to_be_bytes()); // methods
        out.extend(method_flags.to_be_bytes());
        out.extend(name_idx.to_be_bytes());
        out.extend(desc_idx.to_be_bytes());
        out.extend(0u16.to_be_bytes()); // method attrs
        out.extend(0u16.to_be_bytes()); // class attrs
        out
    }

    fn write_class(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let target = dir.join(rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&target).unwrap();
        f.write_all(bytes).unwrap();
        target
    }

    #[test]
    fn parser_detects_exact_static_main() {
        let bytes = class_bytes(&["main", "([Ljava/lang/String;)V", "Foo"], 0x0009, 1, 2);
        assert!(class_has_static_main(&bytes));
        let nonstatic = class_bytes(&["main", "([Ljava/lang/String;)V"], 0x0001, 1, 2);
        assert!(!class_has_static_main(&nonstatic));
        let wrong_desc = class_bytes(&["main", "(Ljava/lang/String;)V"], 0x0009, 1, 2);
        assert!(!class_has_static_main(&wrong_desc));
        let truncated = &bytes[..bytes.len() / 2];
        assert!(!class_has_static_main(truncated));
    }

    #[test]
    fn classes_with_fields_still_parse() {
        // the fixture writes a field on purpose: earlier versions of the
        // scanner mis-truncated at the first non-method member
        let bytes = class_bytes(&["main", "([Ljava/lang/String;)V", "Foo"], 0x0009, 1, 2);
        assert!(class_has_static_main(&bytes));
    }

    #[test]
    fn discover_by_directory_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        let classes = dir.path().join("classes");
        let good = class_bytes(&["main", "([Ljava/lang/String;)V"], 0x0009, 1, 2);
        let other = class_bytes(&["greet", "()V"], 0x0004, 1, 2);
        write_class(&classes, "app/Z.class", &good);
        write_class(&classes, "app/Two.class", &good);
        write_class(&classes, "app/Plain.class", &other);
        let found = discover_main(&classes).unwrap();
        assert_eq!(found, vec!["app.Two", "app.Z"]);
        let absent = dir.path().join("absent");
        assert!(discover_main(&absent).unwrap().is_empty());
    }

    #[test]
    fn decide_main_lists_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let classes = dir.path().join("classes");
        let good = class_bytes(&["main", "([Ljava/lang/String;)V"], 0x0009, 1, 2);
        write_class(&classes, "app/One.class", &good);
        write_class(&classes, "app/Two.class", &good);
        let err = decide_main(&classes, None).unwrap_err();
        assert!(err.message.contains("several classes define a static main"));
        assert!(err.message.contains("app.One"));
        let pinned = Some("app.Pinned".to_string());
        let one = decide_main(&classes, pinned.as_ref()).unwrap();
        assert_eq!(one, "app.Pinned");
    }

    #[test]
    fn none_found_when_classes_absent() {
        let dir = tempfile::tempdir().unwrap();
        let e = decide_main(&dir.path().join("nope"), None).unwrap_err();
        assert!(e.message.contains("no main class found"));
    }

    #[test]
    fn class_names_map_below_the_classes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let classes = dir.path().join("classes");
        write_class(&classes, "app/Verticle.class", b"verticle-bytes");
        assert!(compiled_class_exists(&classes, "app.Verticle"));
        assert!(!compiled_class_exists(&classes, "app.Missing"));
        for bad in ["", "app..Verticle", "app/../Escape", "app\\Escape"] {
            assert!(!compiled_class_exists(&classes, bad), "{bad}");
        }
    }
}
