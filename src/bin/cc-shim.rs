//! This bin target is only used for this crate's tests.
//! It is not intended for users and is not published with the library code to crates.io.

#![cfg_attr(test, allow(dead_code))]
#![allow(clippy::disallowed_methods)]

use std::env;
use std::fs::File;
use std::io::{self, prelude::*};
use std::path::PathBuf;
use std::process::ExitCode;

/// Test-only environment variable naming a directory to record invocations in.
///
/// Arguments are written to the first `out{i}` in it that does not exist yet, so
/// a test reads the `i`-th invocation back with `Test::cmd(i)`. `Test::gcc()`
/// sets this through `Build::env`, and the compile and archive commands that a
/// build actually runs inherit it.
const OUT_DIR: &str = "CC_SHIM_OUT_DIR";

/// Test-only environment variable naming an explicit `PATH`-separated list of
/// files to record invocations in, the first that does not exist yet winning.
///
/// cc sets this on its own probing invocations by renaming the dedicated
/// `CC_SHIM_OUT_FILES_FOR_*` variable for the class of probe it is about to
/// spawn, and clears [`OUT_DIR`] for them at the same time. Which probes run at
/// all depends on the compiler, the target and cc's internal caching, so a test
/// opts one class in by name; every probe it did not ask about records nothing
/// and cannot shift the `out{i}` numbering of the invocations it is asserting
/// on. See `set_probe_env` in `src/command_helpers.rs`.
const OUT_FILES: &str = "CC_SHIM_OUT_FILES";

/// Opts the shim into creating the object and Make-style depfile named by a
/// compile invocation. Used by dependency-tracking integration tests.
const CREATE_COMPILE_OUTPUTS: &str = "CC_SHIM_CREATE_COMPILE_OUTPUTS";

/// Optional additional path to include in a generated Make-style depfile.
const EXTRA_DEPENDENCY: &str = "CC_SHIM_EXTRA_DEPENDENCY";

/// Pick the file this invocation should record its arguments in, if any.
fn out_file(program: &str) -> Option<PathBuf> {
    if let Some(files) = env::var_os(OUT_FILES) {
        let candidate = env::split_paths(&files)
            .filter(|file| !file.as_os_str().is_empty())
            .find(|file| !file.exists());
        return Some(candidate.unwrap_or_else(|| {
            panic!(
                "{}: every file named by {} has already been written to: {:?}",
                program, OUT_FILES, files
            )
        }));
    }

    let out_dir = PathBuf::from(env::var_os(OUT_DIR)?);
    // Find the first nonexistent candidate file to which the program's args can be written.
    Some((0..).find_map(|i| {
        let candidate = out_dir.join(format!("out{i}"));

        if candidate.exists() {
            // If the file exists, commands have already run. Try again.
            None
        } else {
            Some(candidate)
        }
    }).unwrap_or_else(|| panic!("Cannot find the first nonexistent candidate file to which the program's args can be written under out_dir '{}'", out_dir.display())))
}

/// Record the args passed to the command, if this invocation records at all.
fn record(program: &str, args: &[String]) {
    let Some(candidate) = out_file(program) else {
        return;
    };

    let f = File::create(&candidate).unwrap_or_else(|e| {
        panic!(
            "{}: can't create candidate: {}, error: {}",
            program,
            candidate.display(),
            e
        )
    });
    let mut f = io::BufWriter::new(f);

    (|| {
        for arg in args {
            writeln!(f, "{arg}")?;
        }

        f.flush()?;

        let mut f = f.into_inner()?;
        f.flush()?;
        f.sync_all()
    })()
    .unwrap_or_else(|e| {
        panic!(
            "{}: can't write to candidate: {}, error: {}",
            program,
            candidate.display(),
            e
        )
    });
}

fn escape_makefile_path(path: &str) -> String {
    path.replace('\\', "\\\\")
        .replace(' ', "\\ ")
        .replace('#', "\\#")
}

fn create_compile_outputs(args: &[String]) {
    if env::var_os(CREATE_COMPILE_OUTPUTS).is_none() {
        return;
    }

    let argument_after = |flag: &str| {
        args.windows(2)
            .find(|arguments| arguments[0] == flag)
            .map(|arguments| arguments[1].as_str())
    };
    let (Some(object), Some(depfile), Some(source)) =
        (argument_after("-o"), argument_after("-MF"), args.last())
    else {
        return;
    };

    File::create(object)
        .unwrap_or_else(|error| panic!("can't create compiler output {object}: {error}"));

    let mut dependencies = vec![source.clone()];
    if let Some(extra) = env::var_os(EXTRA_DEPENDENCY) {
        dependencies.push(extra.to_string_lossy().into_owned());
    }
    let dependencies = dependencies
        .iter()
        .map(|path| escape_makefile_path(path))
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        depfile,
        format!("{}: {dependencies}\n", escape_makefile_path(object)),
    )
    .unwrap_or_else(|error| panic!("can't create dependency file {depfile}: {error}"));
}

fn main() -> ExitCode {
    let argv = env::args().collect::<Vec<_>>();
    let (program, args) = argv.split_first().expect("Unexpected empty args");

    record(program, args);

    // Compiler family detection preprocesses a probe file with `-E` and reads
    // the family out of the expansion. The shim is not a preprocessor and
    // cannot answer that, so it declines and cc falls back to deducing the
    // family from the compiler's name, which is what every test here expects.
    //
    // This used to happen by accident: detection ran with a fresh environment,
    // so the shim panicked on the missing `CC_SHIM_OUT_DIR` instead. It is
    // spelled out now that detection runs in the environment the compile
    // commands run in and reaches the shim on purpose.
    if args.iter().any(|a| a.as_str() == "-E") {
        eprintln!("{program}: the shim cannot preprocess");
        return ExitCode::FAILURE;
    }

    if program.starts_with("clang") {
        // Validate that we got no `-?` without a preceding `--driver-mode=cl`. Compiler family
        // detection depends on this.
        if let Some(cl_like_help_option_idx) = args.iter().position(|a| a.as_str() == "-?") {
            let has_cl_clang_driver_before_cl_like_help_option = args
                .iter()
                .take(cl_like_help_option_idx)
                .rev()
                .find_map(|a| a.strip_prefix("--driver-mode="))
                == Some("cl");
            if has_cl_clang_driver_before_cl_like_help_option {
                return ExitCode::SUCCESS;
            } else {
                eprintln!(
                    "Found `-?` argument, but it was not preceded by a `--driver-mode=cl` argument."
                );
                return ExitCode::FAILURE;
            }
        }
    }

    // Allow tests to make the shim fail when a specific arg is present.
    if let Ok(fail_arg) = env::var("CC_SHIM_FAIL_IF_ARG") {
        if args.iter().any(|a| a.as_str() == fail_arg) {
            eprintln!("{program}: simulated failure for arg '{fail_arg}'");
            return ExitCode::FAILURE;
        }
    }

    create_compile_outputs(args);

    // Create a file used by some tests. Only the invocations a build actually
    // performs are asked to do this; probes are not archiving anything.
    if let Some(out_dir) = env::var_os(OUT_DIR) {
        let path = &PathBuf::from(out_dir).join("libfoo.a");
        File::create(path).unwrap_or_else(|e| {
            panic!(
                "{}: can't create libfoo.a: {}, error: {}",
                program,
                path.display(),
                e
            )
        });
    }

    ExitCode::SUCCESS
}
