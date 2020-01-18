use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Output;

use super::{TestKind, Runner, Test};
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
        let num_kinds = (project.has_pass as u8)
            + (project.has_compile_fail as u8)
            + (project.has_output as u8);

        let show_expected = num_kinds > 1;
        message::begin_test(test, show_expected);
        check_exists(&test.path)?;

        let output = self.runner.build(test)
            .map_err(|e| Error::External(e.to_string()))?;

        let build_stderr = normalize::diagnostics(&output.stderr, test, project);
        let check = match test.kind {
            TestKind::Pass => Test::check_pass,
            TestKind::CompileFail => Test::check_compile_fail,
            TestKind::Output => Test::check_output,
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

    fn check_output<R: TestRunner>(
        &self,
        runner: &mut R,
        project: &Project,
        build_output: Output,
        variations: Variations,
    ) -> Result<()> {
        let preferred = variations.preferred();
        if !build_output.status.success() {
            message::failed_to_build(preferred);
            return Err(Error::BuildFail);
        }

        let output = runner.run(self)
            .map_err(|e| Error::External(e.to_string()))?;

        println!(); println!();
        let stderr_path = self.path.with_extension("stderr");
        message::output_prefix("stderr");
        check_output(self, project, &stderr_path, false, &output.stderr)?;

        let stdout_path = self.path.with_extension("stdout");
        message::output_prefix("stdout");
        check_output(self, project, &stdout_path, false, &output.stdout).map(|_| ())?;

        println!();
        Ok(())
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

        // FIXME: This is different than what was here before...
        // Before, it used `preferred`, now, it uses stderr directly.
        let stderr_path = self.path.with_extension("stderr");
        match check_output(self, project, &stderr_path, false, &build_output.stderr) {
            Ok(true) => {
                message::fail_output(Warn, &build_output.stdout);
                return Ok(());
            }
            result => result.map(|_| ())
        }
    }
}

fn check_output(
    test: &Test,
    project: &Project,
    path: &Path,
    must_exist: bool,
    output: &[u8]
) -> Result<bool> {
    let content = normalize::diagnostics(output, test, project);
    if !path.exists() && (must_exist || !output.is_empty()) {
        make_wip(project, path, content.preferred())?;
        return Ok(true);
    }

    let expected = if path.exists() {
        let expected = fs::read_to_string(path)
            .map_err(Error::ReadStderr)? // FIXME
            .replace("\r\n", "\n");

        if content.any(|v| expected == v) {
            message::ok();
            return Ok(false);
        }

        expected
    } else if output.is_empty() {
        message::ok();
        return Ok(false);
    } else {
        "".into()
    };

    let actual = content.preferred();
    match project.update {
        Update::Wip => {
            message::mismatch(&expected, actual);
            Err(Error::Mismatch)
        }
        Update::Overwrite => {
            message::overwrite(path, actual);
            fs::write(path, actual).map_err(Error::WriteStderr)?; // FIXME
            Ok(false)
        }
    }
}

fn make_wip(project: &Project, path: &Path, content: &str) -> Result<()> {
    let ext = path.extension().expect("wip path has extension");
    match project.update {
        Update::Wip => {
            let wip_dir = Path::new("wip");
            fs::create_dir_all(wip_dir)?;
            let gitignore_path = wip_dir.join(".gitignore");
            fs::write(gitignore_path, "*\n")?;

            let default = Path::new("test").with_extension(ext);
            let name = path.file_name()
                .unwrap_or_else(|| default.as_os_str());
            let wip_path = wip_dir.join(name);
            message::write_wip(&wip_path, &path, content);
            fs::write(wip_path, content).map_err(Error::WriteStderr)?;
        }
        Update::Overwrite => {
            message::overwrite(&path, content);
            fs::write(path, content).map_err(Error::WriteStderr)?;
        }
    }

    Ok(())
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
                                kind: test.kind
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
