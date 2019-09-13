use termcolor::Color::{self, *};

use super::{TestKind, Test};
use crate::error::Error;
use crate::normalize;
use crate::term;

use std::path::Path;
use std::process::Output;

pub(crate) enum Level {
    Fail,
    Warn,
}

pub(crate) use self::Level::*;

pub(crate) fn prepare_fail(err: Error) {
    if err.already_printed() {
        return;
    }

    term::bold_color(Red);
    print!("ERROR");
    term::reset();
    println!(": {}", err);
    println!();
}

pub(crate) fn test_fail(err: Error) {
    if err.already_printed() {
        return;
    }

    term::bold_color(Red);
    println!("error");
    term::color(Red);
    println!("{}", err);
    term::reset();
    println!();
}

pub(crate) fn no_tests_enabled() {
    term::color(Yellow);
    println!("There are no tests enabled yet.");
    term::reset();
}

pub(crate) fn ok() {
    term::color(Green);
    println!("ok");
    term::reset();
}

pub(crate) fn begin_test(test: &Test, show_expected: bool) {
    let display_name = if show_expected {
        test.path
            .file_name()
            .unwrap_or_else(|| test.path.as_os_str())
            .to_string_lossy()
    } else {
        test.path.as_os_str().to_string_lossy()
    };

    print!("test ");
    term::bold();
    print!("{}", display_name);
    term::reset();

    if show_expected {
        match test.kind {
            TestKind::Pass => print!(" [should pass]"),
            TestKind::CompileFail => print!(" [should fail to compile]"),
            TestKind::Output => print!(" [should produce output]"),
        }
    }

    print!(" ... ");
}

pub(crate) fn failed_to_build(stderr: &str) {
    term::bold_color(Red);
    println!("error");
    snippet(Red, stderr);
    println!();
}

pub(crate) fn should_not_have_compiled() {
    term::bold_color(Red);
    println!("error");
    term::color(Red);
    println!("TestKind test case to fail to compile, but it succeeded.");
    term::reset();
    println!();
}

pub(crate) fn output_prefix(kind: &str) {
    term::bold_color(Blue);
    print!("{}", kind);
    term::reset();
    print!(" ... ");
}

pub(crate) fn write_wip(wip_path: &Path, path: &Path, content: &str) {
    let wip_path = wip_path.to_string_lossy();
    let path = path.to_string_lossy();

    term::bold_color(Yellow);
    println!("wip");
    println!();
    print!("NOTE");
    term::reset();
    println!(": writing the following output to `{}`.", wip_path);
    println!(
        "Move this file to `{}` to accept it as correct.",
        path,
    );
    snippet(Yellow, content);
    println!();
}

pub(crate) fn overwrite(path: &Path, content: &str) {
    let path = path.to_string_lossy();

    term::bold_color(Yellow);
    println!("wip");
    println!();
    print!("NOTE");
    term::reset();
    println!(": writing the following output to `{}`.", path);
    snippet(Yellow, content);
    println!();
}

pub(crate) fn mismatch(expected: &str, actual: &str) {
    term::bold_color(Red);
    println!("mismatch");
    term::reset();
    println!();
    term::bold_color(Blue);
    println!("EXPECTED:");
    snippet(Blue, expected);
    println!();
    term::bold_color(Red);
    println!("ACTUAL OUTPUT:");
    snippet(Red, actual);
    println!();
    term::bold_color(Magenta);
    print!("DIFF:");
    diff(expected, actual);
    println!();
}

pub(crate) fn output(warnings: &str, output: &Output) {
    let success = output.status.success();
    let stdout = normalize::trim(&output.stdout);
    let stderr = normalize::trim(&output.stderr);
    let has_output = !stdout.is_empty() || !stderr.is_empty();

    if success {
        ok();
        if has_output || !warnings.is_empty() {
            println!();
        }
    } else {
        term::bold_color(Red);
        println!("error");
        term::color(Red);
        if has_output {
            println!("Test case failed at runtime.");
        } else {
            println!("Execution of the test case was unsuccessful but there was no output.");
        }
        term::reset();
        println!();
    }

    self::warnings(warnings);

    let color = if success { Yellow } else { Red };

    for (name, content) in &[("STDOUT", stdout), ("STDERR", stderr)] {
        if !content.is_empty() {
            term::bold_color(color);
            println!("{}:", name);
            snippet(color, &normalize::trim(content));
            println!();
        }
    }
}

pub(crate) fn fail_output(level: Level, stdout: &[u8]) {
    let color = match level {
        Fail => Red,
        Warn => Yellow,
    };

    if !stdout.is_empty() {
        term::bold_color(color);
        println!("STDOUT:");
        snippet(color, &normalize::trim(stdout));
        println!();
    }
}

pub(crate) fn warnings(warnings: &str) {
    if warnings.is_empty() {
        return;
    }

    term::bold_color(Yellow);
    println!("WARNINGS:");
    snippet(Yellow, warnings);
    println!();
}

pub(crate) fn dotted_line() {
    println!("{}", "┈".repeat(60));
}

fn snippet(color: Color, content: &str) {
    term::color(color);
    dotted_line();

    // Color one line at a time because Travis does not preserve color setting
    // across output lines.
    for line in content.lines() {
        term::color(color);
        println!("{}", line);
    }

    term::color(color);
    dotted_line();
    term::reset();
}

fn diff(expected: &str, actual: &str) {
    use diff::Result as Diff;

    term::color(Red);
    print!(" -expected ");
    term::color(Green);
    println!("+actual ");

    term::bold_color(Magenta);
    dotted_line();

    for diff in diff::lines(expected, actual) {
        match diff {
            Diff::Both(x, _) => {
                term::reset();
                println!(" {}", x);
            }
            Diff::Right(x) => {
                term::color(Green);
                println!("+{}", x);
            }
            Diff::Left(x) => {
                term::color(Red);
                println!("-{}", x);
            }
        }
    }

    term::bold_color(Magenta);
    dotted_line();
    term::reset();
}
