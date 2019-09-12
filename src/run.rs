use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Output;

use super::{Expected, Runner, Test};
use crate::cargo::{prepare_project, Project};
use crate::env::Update;
use crate::error::{Error, Result};
use crate::message::{self, Fail, Warn};
use crate::normalize::{self, Variations};

use crate::TestRunner;

impl<R: TestRunner> Runner<R> {
    pub fn run(&mut self) {
        let (mut tests, bad_tests) = expand_globs(&self.tests);
        filter(&mut tests);

        let mut failures = 0;
        for (test, error) in bad_tests {
            message::begin_test(&test, false);
            message::test_fail(error);
            failures += 1;
        }

        let (project, _) = prepare_project(&tests).unwrap_or_else(|err| {
            message::prepare_fail(err);
            panic!("tests failed");
        });

        self.runner.prepare(&tests).unwrap_or_else(|err| {
            message::prepare_fail(Error::External(err.to_string()));
            panic!("tests failed");
        });

        print!("\n\n");

        if tests.is_empty() {
            message::no_tests_enabled();
        } else {
            for test in &tests {
                if let Err(e) = self.run_one(test, &project) {
                    message::test_fail(e);
                    failures += 1;
                }
            }
        }

        print!("\n\n");

        if failures > 0 && project.name != "trybuild-tests" {
            panic!("{} of {} tests failed", failures, tests.len());
        }
    }

    fn run_one(&mut self, test: &Test, project: &Project) -> Result<()> {
        let show_expected = project.has_pass && project.has_compile_fail;
        message::begin_test(test, show_expected);
        check_exists(&test.path)?;

        let output = self.runner.build(test)
            .map_err(|e| Error::External(e.to_string()))?;

        let build_stderr = normalize::diagnostics(&output.stderr).map(|stderr| {
            stderr.replace(&test.name, "$CRATE")
                .replace(&*project.source_dir.to_string_lossy(), "$DIR")
        });

        let check = match test.expected {
            Expected::Pass => Test::check_pass,
            Expected::CompileFail => Test::check_compile_fail,
        };

        check(test, &mut self.runner, project, output, build_stderr)
    }
}

impl Test {
    fn check_pass<R: TestRunner>(
        &self,
        runner: &mut R,
        _project: &Project,
        build_output: Output,
        variations: Variations,
    ) -> Result<()> {
        let preferred = variations.preferred();
        if !build_output.status.success() {
            message::failed_to_build(preferred);
            return Err(Error::CargoFail);
        }

        let mut output = runner.run(self)
            .map_err(|e| Error::External(e.to_string()))?;

        output.stdout.splice(..0, build_output.stdout);
        message::output(preferred, &output);
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::RunFailed)
        }
    }

    fn check_compile_fail<R: TestRunner>(
        &self,
        _runner: &mut R,
        project: &Project,
        build_output: Output,
        variations: Variations,
    ) -> Result<()> {
        let preferred = variations.preferred();

        if build_output.status.success() {
            message::should_not_have_compiled();
            message::fail_output(Fail, &build_output.stdout);
            message::warnings(preferred);
            return Err(Error::ShouldNotHaveCompiled);
        }

        let stderr_path = self.path.with_extension("stderr");

        if !stderr_path.exists() {
            match project.update {
                Update::Wip => {
                    let wip_dir = Path::new("wip");
                    fs::create_dir_all(wip_dir)?;
                    let gitignore_path = wip_dir.join(".gitignore");
                    fs::write(gitignore_path, "*\n")?;
                    let stderr_name = stderr_path
                        .file_name()
                        .unwrap_or_else(|| OsStr::new("test.stderr"));
                    let wip_path = wip_dir.join(stderr_name);
                    message::write_stderr_wip(&wip_path, &stderr_path, preferred);
                    fs::write(wip_path, preferred).map_err(Error::WriteStderr)?;
                }
                Update::Overwrite => {
                    message::overwrite_stderr(&stderr_path, preferred);
                    fs::write(stderr_path, preferred).map_err(Error::WriteStderr)?;
                }
            }
            message::fail_output(Warn, &build_output.stdout);
            return Ok(());
        }

        let expected = fs::read_to_string(&stderr_path)
            .map_err(Error::ReadStderr)?
            .replace("\r\n", "\n");

        if variations.any(|stderr| expected == stderr) {
            message::ok();
            return Ok(());
        }

        match project.update {
            Update::Wip => {
                message::mismatch(&expected, preferred);
                Err(Error::Mismatch)
            }
            Update::Overwrite => {
                message::overwrite_stderr(&stderr_path, preferred);
                fs::write(stderr_path, preferred).map_err(Error::WriteStderr)?;
                Ok(())
            }
        }
    }
}

fn check_exists(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    match File::open(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::Open(path.to_owned(), err)),
    }
}

fn expand_globs(tests: &[Test]) -> (Vec<Test>, Vec<(Test, Error)>) {
    fn glob(pattern: &str) -> Result<Vec<PathBuf>> {
        let mut paths = glob::glob(pattern)?
            .map(|entry| entry.map_err(Error::from))
            .collect::<Result<Vec<PathBuf>>>()?;
        paths.sort();
        Ok(paths)
    }

    let mut expanded_tests = Vec::new();
    let mut bad_tests = Vec::new();

    for test in tests {
        if let Some(utf8) = test.path.to_str() {
            if utf8.contains('*') {
                match glob(utf8) {
                    Ok(paths) => {
                        for path in paths {
                            let num = expanded_tests.len();
                            let name = format!("{}-{:03}", test.name, num);
                            expanded_tests.push(Test {
                                name,
                                path,
                                expected: test.expected
                            });
                        }
                    }
                    Err(error) => {
                        bad_tests.push((test.clone(), error));
                    }
                }
            } else {
                expanded_tests.push(test.clone());
            }
        }
    }

    (expanded_tests, bad_tests)
}

// Filter which test cases are run by trybuild.
//
//     $ cargo test -- ui trybuild=tuple_structs.rs
//
// The first argument after `--` must be the trybuild test name i.e. the name of
// the function that has the #[test] attribute and calls trybuild. That's to get
// Cargo to run the test at all. The next argument starting with `trybuild=`
// provides a filename filter. Only test cases whose filename contains the
// filter string will be run.
fn filter(tests: &mut Vec<Test>) {
    let filters = env::args_os()
        .flat_map(OsString::into_string)
        .filter_map(|mut arg| {
            const PREFIX: &str = "trybuild=";
            if arg.starts_with(PREFIX) && arg != PREFIX {
                Some(arg.split_off(PREFIX.len()))
            } else {
                None
            }
        })
        .collect::<Vec<String>>();

    if filters.is_empty() {
        return;
    }

    tests.retain(|t| {
        filters
            .iter()
            .any(|f| t.path.to_string_lossy().contains(f))
    });
}
