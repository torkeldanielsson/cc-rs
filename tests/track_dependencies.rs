#![allow(missing_docs)]
#![cfg(feature = "track-dependencies")]

mod support;

use std::fs;

use support::Test;

#[test]
fn gnu_reuses_current_object_and_rebuilds_for_changed_header() {
    let mut test = Test::gnu();
    let source = test.td.path().join("source.c");
    let header = test.td.path().join("header with space.h");
    fs::write(&source, "#include \"header with space.h\"\n").unwrap();
    fs::write(&header, "#define VALUE 1\n").unwrap();
    test.env.set("CC_SHIM_CREATE_COMPILE_OUTPUTS", "1");
    test.env.set("CC_SHIM_EXTRA_DEPENDENCY", &header);

    let compile = || {
        test.gcc()
            .track_dependencies(true)
            .file(&source)
            .compile("foo");
    };

    compile();
    test.cmd(0)
        .must_have("-MD")
        .must_have("-MF")
        .must_have(&source);
    test.cmd(1).must_not_have(&source);
    test.cmd(2).must_not_have(&source);

    compile();
    test.cmd(3).must_not_have(&source);
    test.cmd(4).must_not_have(&source);
    assert!(
        !test.td.path().join("out5").exists(),
        "the second build should only run the two archiver commands"
    );

    fs::write(&header, "#define VALUE 2\n").unwrap();
    compile();
    test.cmd(5).must_have(&source);
    test.cmd(6).must_not_have(&source);
    test.cmd(7).must_not_have(&source);
}

#[test]
fn changed_compiler_command_rebuilds_object() {
    let mut test = Test::gnu();
    let source = test.td.path().join("source.c");
    fs::write(&source, "int value;\n").unwrap();
    test.env.set("CC_SHIM_CREATE_COMPILE_OUTPUTS", "1");

    test.gcc()
        .track_dependencies(true)
        .file(&source)
        .compile("foo");
    test.gcc()
        .track_dependencies(true)
        .flag("-DCHANGED")
        .file(&source)
        .compile("foo");

    test.cmd(3).must_have("-DCHANGED").must_have(&source);
}

#[test]
fn native_msvc_uses_source_dependencies_json() {
    let test = Test::msvc();
    test.gcc()
        .track_dependencies(true)
        .file("foo.c")
        .compile("foo");

    let command = test.cmd(0);
    command.must_have("/sourceDependencies");
    assert!(command
        .args
        .iter()
        .any(|argument| argument.ends_with(".json")));
}

#[test]
fn clang_cl_forwards_make_depfile_flags_to_clang_driver() {
    let test = Test::msvc();
    test.shim("clang-cl.exe");
    test.gcc()
        .compiler(test.td.path().join("clang-cl.exe"))
        .track_dependencies(true)
        .file("foo.c")
        .compile("foo");

    let command = test.cmd(0);
    command.must_have("/clang:-MD");
    assert!(command
        .args
        .iter()
        .any(|argument| argument.starts_with("/clang:-MF") && argument.ends_with(".dep")));
}

#[test]
fn assembly_is_not_given_c_or_cpp_dependency_flags() {
    let test = Test::gnu();
    test.gcc()
        .track_dependencies(true)
        .file("foo.S")
        .compile("foo");

    test.cmd(0).must_not_have("-MD").must_not_have("-MF");
}

#[test]
fn tracking_is_disabled_by_default() {
    let test = Test::gnu();
    test.gcc().file("foo.c").compile("foo");

    test.cmd(0).must_not_have("-MD").must_not_have("-MF");
}
