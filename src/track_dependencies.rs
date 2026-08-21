use crate::{Object, ToolFamily};
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug)]
enum DependencyFileFormat {
    Makefile,
    MsvcJson,
}

#[derive(Clone, Debug)]
pub(crate) struct DependencyFile {
    path: PathBuf,
    format: DependencyFileFormat,
}

impl DependencyFile {
    pub(crate) fn for_compiler(object: &Object, family: ToolFamily) -> Self {
        if matches!(family, ToolFamily::Msvc { clang_cl: false }) {
            Self {
                path: object.dst.with_extension("json"),
                format: DependencyFileFormat::MsvcJson,
            }
        } else {
            Self {
                path: object.dst.with_extension("dep"),
                format: DependencyFileFormat::Makefile,
            }
        }
    }

    pub(crate) fn add_compiler_flags(&self, command: &mut Command, family: ToolFamily) {
        match family {
            ToolFamily::Msvc { clang_cl: false } => {
                command.arg("/sourceDependencies").arg(&self.path);
            }
            ToolFamily::Msvc { clang_cl: true } => {
                // In clang-cl mode, a plain `-MD` selects the dynamic MSVC
                // runtime. Forward the Clang driver dependency flags through
                // `/clang:` so they retain their GCC-compatible meaning.
                command.arg("/clang:-MD");
                let mut output = OsString::from("/clang:-MF");
                output.push(&self.path);
                command.arg(output);
            }
            ToolFamily::Gnu | ToolFamily::Clang { .. } => {
                command.arg("-MD").arg("-MF").arg(&self.path);
            }
        }
    }

    pub(crate) fn dependencies(&self, object: &Object) -> Option<Vec<PathBuf>> {
        let contents = fs::read_to_string(&self.path).ok()?;
        let mut dependencies = match self.format {
            DependencyFileFormat::Makefile => parse_makefile_depfile(&contents)?,
            DependencyFileFormat::MsvcJson => parse_msvc_depfile(&contents)?,
        };

        if !dependencies.iter().any(|path| path == &object.src) {
            dependencies.push(object.src.clone());
        }
        Some(dependencies)
    }
}

pub(crate) struct CommandStamp {
    path: PathBuf,
    contents: String,
}

impl CommandStamp {
    pub(crate) fn new(object: &Object, command: &Command) -> Self {
        Self {
            path: object.dst.with_extension("command"),
            contents: format!("{command:?}"),
        }
    }

    fn changed(&self) -> bool {
        fs::read_to_string(&self.path).map_or(true, |contents| contents != self.contents)
    }

    /// Record the command only after compilation succeeded. If persisting the
    /// stamp fails, the next invocation safely recompiles the object.
    pub(crate) fn persist(&self) {
        if !self.changed() {
            return;
        }
        let _ = fs::write(&self.path, &self.contents);
    }
}

pub(crate) fn is_run_needed(
    object: &Object,
    dependency_file: &DependencyFile,
    command_stamp: &CommandStamp,
) -> bool {
    if command_stamp.changed() {
        return true;
    }

    let Some(output_time) = modified_time(&object.dst) else {
        return true;
    };
    let Some(dependencies) = dependency_file.dependencies(object) else {
        return true;
    };

    dependencies.into_iter().any(|dependency| {
        modified_time(dependency).map_or(true, |input_time| input_time >= output_time)
    })
}

pub(crate) fn canonical_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn external_dependency_identity(
    dependency: &Path,
    canonical_out_dir: &Path,
) -> Option<PathBuf> {
    let identity = canonical_path(dependency);
    (!identity.starts_with(canonical_out_dir)).then_some(identity)
}

fn modified_time(path: impl AsRef<Path>) -> Option<SystemTime> {
    File::open(path).ok()?.metadata().ok()?.modified().ok()
}

fn parse_msvc_depfile(contents: &str) -> Option<Vec<PathBuf>> {
    let parsed = json::parse(contents).ok()?;
    let data = &parsed["Data"];
    let includes = &data["Includes"];
    if !includes.is_array() {
        return None;
    }

    let mut dependencies = Vec::new();
    if let Some(source) = data["Source"].as_str() {
        dependencies.push(source.into());
    }
    dependencies.extend(
        includes
            .members()
            .filter_map(|value| value.as_str())
            .map(PathBuf::from),
    );
    Some(dependencies)
}

fn parse_makefile_depfile(contents: &str) -> Option<Vec<PathBuf>> {
    let separator = find_rule_separator(contents)?;
    let dependencies = parse_makefile_words(&contents[separator + 1..]);
    if dependencies.is_empty() {
        None
    } else {
        Some(dependencies)
    }
}

fn find_rule_separator(contents: &str) -> Option<usize> {
    let bytes = contents.as_bytes();
    let mut escaped = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte != b':' {
            continue;
        }

        // Do not mistake a Windows drive prefix for the rule separator.
        let drive_prefix = index > 0
            && bytes[index - 1].is_ascii_alphabetic()
            && (index == 1 || bytes[index - 2].is_ascii_whitespace())
            && bytes
                .get(index + 1)
                .map_or(false, |next| matches!(next, b'/' | b'\\'));
        if !drive_prefix {
            return Some(index);
        }
    }
    None
}

fn parse_makefile_words(contents: &str) -> Vec<PathBuf> {
    let mut dependencies = Vec::new();
    let mut word = String::new();
    let mut chars = contents.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(escaped) => word.push(escaped),
                None => word.push('\\'),
            },
            '#' => break,
            whitespace if whitespace.is_whitespace() => {
                if !word.is_empty() {
                    dependencies.push(PathBuf::from(std::mem::take(&mut word)));
                }
            }
            other => word.push(other),
        }
    }

    if !word.is_empty() {
        dependencies.push(word.into());
    }
    dependencies
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_path, external_dependency_identity, find_rule_separator, parse_makefile_depfile,
        parse_msvc_depfile,
    };
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn parses_makefile_dependencies_and_escapes() {
        let dependencies = parse_makefile_depfile(
            "out.o: src/main.c include/with\\ space.h \\\n             include/next.h include/hash\\#name.h\n",
        )
        .unwrap();

        assert_eq!(
            dependencies,
            [
                PathBuf::from("src/main.c"),
                PathBuf::from("include/with space.h"),
                PathBuf::from("include/next.h"),
                PathBuf::from("include/hash#name.h"),
            ]
        );
    }

    #[test]
    fn finds_rule_separator_after_windows_drive_prefix() {
        assert_eq!(find_rule_separator("C:/out/foo.o: C:/src/foo.c"), Some(12));
    }

    #[test]
    fn parses_msvc_source_dependencies() {
        let dependencies = parse_msvc_depfile(
            r#"{
                "Version": "1.2",
                "Data": {
                    "Source": "C:\\src\\main.cpp",
                    "Includes": ["C:\\include\\one.h", "C:\\with space\\two.h"]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            dependencies,
            [
                PathBuf::from(r"C:\src\main.cpp"),
                PathBuf::from(r"C:\include\one.h"),
                PathBuf::from(r"C:\with space\two.h"),
            ]
        );
    }

    #[test]
    fn excludes_generated_dependencies_under_out_dir() {
        let temporary = tempfile::tempdir().unwrap();
        let out_dir = temporary.path().join("out");
        let generated = out_dir.join("generated.h");
        let source = temporary.path().join("source.h");
        fs::create_dir(&out_dir).unwrap();
        fs::write(&generated, "generated").unwrap();
        fs::write(&source, "source").unwrap();

        let out_dir = canonical_path(&out_dir);
        assert_eq!(external_dependency_identity(&generated, &out_dir), None);
        assert_eq!(
            external_dependency_identity(&source, &out_dir),
            Some(canonical_path(&source))
        );
    }
}
