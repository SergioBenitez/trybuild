use crate::Test;
use crate::cargo::Project;

pub fn trim<S: AsRef<[u8]>>(output: S) -> String {
    let bytes = output.as_ref();
    let mut normalized = String::from_utf8_lossy(bytes).to_string();

    let len = normalized.trim_end().len();
    normalized.truncate(len);

    if !normalized.is_empty() {
        normalized.push('\n');
    }

    normalized
}

pub fn diagnostics(output: &[u8], test: &Test, project: &Project) -> Variations {
    let mut from_bytes = String::from_utf8_lossy(&output).to_string();
    from_bytes = from_bytes.replace("\r\n", "\n")
            .replace(&test.name, "$CRATE");

    let source_dir = project.source_dir.to_string_lossy();
    let variations = [Basic, StripCouldNotCompile]
        .iter()
        .map(|normalization| apply(&from_bytes, *normalization, &source_dir))
        .collect();

    Variations { variations }
}

pub struct Variations {
    variations: Vec<String>,
}

impl Variations {
    pub fn preferred(&self) -> &str {
        self.variations.last().unwrap()
    }

    pub fn any<F: FnMut(&str) -> bool>(&self, mut f: F) -> bool {
        self.variations.iter().any(|stderr| f(stderr))
    }
}

#[derive(PartialOrd, PartialEq, Copy, Clone)]
enum Normalization {
    Basic,
    StripCouldNotCompile,
}

use self::Normalization::*;

fn apply(original: &str, normalization: Normalization, source_dir: &str) -> String {
    let mut normalized = String::new();

    for line in original.lines() {
        if let Some(line) = filter(line, normalization) {
            let line = line.trim_end();
            if line.contains(&*source_dir) {
                if cfg!(windows) {
                    normalized += &line
                        .replace(&*source_dir, "$DIR")
                        .replace('\\', "/")
                        .replace("//?/", "");
                } else {
                    normalized += &line.replace(&*source_dir, "$DIR");
                }
            } else {
                normalized += &line;
            }

            if !normalized.ends_with("\n\n") {
                normalized.push('\n');
            }
        }
    }

    trim(normalized)
}

fn filter(line: &str, normalization: Normalization) -> Option<String> {
    if line.trim_start().starts_with("--> ") {
        if let Some(cut_end) = line.rfind(&['/', '\\'][..]) {
            let cut_start = line.find('>').unwrap() + 2;
            return Some(line[..cut_start].to_owned() + "$DIR/" + &line[cut_end + 1..]);
        }
    }

    if line.starts_with("error: aborting due to ") {
        return None;
    }

    if line == "To learn more, run the command again with --verbose." {
        return None;
    }

    if normalization >= StripCouldNotCompile {
        if line.starts_with("error: Could not compile `") {
            return None;
        }
    }

    Some(line.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalization() {
        let s = super::apply(
            "C:\\\\foo\\bar",
            super::Normalization::StripCouldNotCompile,
            "C:\\\\foo\\bar");
        assert_eq!(s, "$DIR\n")
    }
}
