use serde::{Deserialize, Serialize};

pub const MANIFESTS: [&str; 6] = [
    "Cargo.toml",
    "package.json",
    "composer.json",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub section: String,
    pub file: String,
    pub line: usize,
}

fn line_of(text: &str, name: &str, after: &str) -> usize {
    let start = text
        .lines()
        .position(|l| l.contains(after))
        .unwrap_or_default();
    text.lines()
        .enumerate()
        .skip(start)
        .find(|(_, l)| {
            let l = l.trim_start().trim_start_matches(['"', '\'']);
            l.starts_with(name)
        })
        .map_or(start, |(n, _)| n)
}

fn toml_version(value: &toml::Value) -> String {
    match value {
        toml::Value::String(v) => v.clone(),
        toml::Value::Table(t) => t
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .or_else(|| t.get("path").map(|_| "path".to_owned()))
            .or_else(|| t.get("git").map(|_| "git".to_owned()))
            .or_else(|| t.get("workspace").map(|_| "workspace".to_owned()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn cargo(text: &str) -> Option<Vec<Dependency>> {
    let table: toml::Table = text.parse().ok()?;
    let mut out = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(entries) = table.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in entries {
            out.push(Dependency {
                name: name.clone(),
                version: toml_version(value),
                section: section.to_owned(),
                file: "Cargo.toml".to_owned(),
                line: line_of(text, name, &format!("[{section}]")),
            });
        }
    }
    Some(out)
}

fn json_sections(text: &str, file: &str, sections: &[&str]) -> Option<Vec<Dependency>> {
    let root: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = Vec::new();
    for section in sections {
        let Some(entries) = root.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, value) in entries {
            out.push(Dependency {
                name: name.clone(),
                version: value.as_str().unwrap_or_default().to_owned(),
                section: (*section).to_owned(),
                file: file.to_owned(),
                line: line_of(text, name, &format!("\"{section}\"")),
            });
        }
    }
    Some(out)
}

fn go_mod(text: &str) -> Vec<Dependency> {
    let mut out = Vec::new();
    let mut in_block = false;
    for (line, raw) in text.lines().enumerate() {
        let code = raw.split("//").next().unwrap_or_default().trim();
        let entry = if code == "require (" {
            in_block = true;
            continue;
        } else if code == ")" {
            in_block = false;
            continue;
        } else if let Some(one) = code.strip_prefix("require ") {
            one
        } else if in_block {
            code
        } else {
            continue;
        };
        let mut words = entry.split_whitespace();
        if let (Some(name), Some(version)) = (words.next(), words.next()) {
            out.push(Dependency {
                name: name.to_owned(),
                version: version.to_owned(),
                section: if raw.contains("// indirect") {
                    "indirect"
                } else {
                    "require"
                }
                .to_owned(),
                file: "go.mod".to_owned(),
                line,
            });
        }
    }
    out
}

fn requirement(spec: &str) -> Option<(String, String)> {
    let spec = spec.split(';').next()?.trim();
    let end = spec.find(|c: char| !(c.is_alphanumeric() || "-_.".contains(c)))?;
    let spec_end = if end == 0 { return None } else { end };
    let version = spec[spec_end..]
        .trim_start_matches(|c| c != '=' && c != '<' && c != '>' && c != '~' && c != '!');
    Some((spec[..spec_end].to_owned(), version.trim().to_owned()))
}

fn requirement_or_bare(spec: &str) -> Option<(String, String)> {
    let spec = spec.trim();
    let bare = !spec.is_empty()
        && spec
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.".contains(c));
    if bare {
        Some((spec.to_owned(), String::new()))
    } else {
        requirement(spec)
    }
}

fn requirements(text: &str) -> Vec<Dependency> {
    text.lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let code = raw.split('#').next()?.trim();
            if code.starts_with('-') {
                return None;
            }
            let (name, version) = requirement_or_bare(code)?;
            Some(Dependency {
                name,
                version,
                section: "requirements".to_owned(),
                file: "requirements.txt".to_owned(),
                line,
            })
        })
        .collect()
}

fn pyproject(text: &str) -> Option<Vec<Dependency>> {
    let table: toml::Table = text.parse().ok()?;
    let mut out = Vec::new();
    let project = table.get("project").and_then(toml::Value::as_table);
    for spec in project
        .and_then(|p| p.get("dependencies"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
    {
        if let Some((name, version)) = requirement_or_bare(spec) {
            out.push(Dependency {
                line: line_of(text, &name, "dependencies"),
                name,
                version,
                section: "dependencies".to_owned(),
                file: "pyproject.toml".to_owned(),
            });
        }
    }
    let poetry = table
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(toml::Value::as_table);
    for (name, value) in poetry.into_iter().flatten() {
        out.push(Dependency {
            name: name.clone(),
            version: toml_version(value),
            section: "poetry".to_owned(),
            file: "pyproject.toml".to_owned(),
            line: line_of(text, name, "[tool.poetry.dependencies]"),
        });
    }
    Some(out)
}

pub fn parse(file: &str, text: &str) -> Result<Vec<Dependency>, String> {
    let parsed = match file {
        "Cargo.toml" => cargo(text),
        "package.json" => json_sections(
            text,
            file,
            &["dependencies", "devDependencies", "peerDependencies"],
        ),
        "composer.json" => json_sections(text, file, &["require", "require-dev"]),
        "go.mod" => Some(go_mod(text)),
        "pyproject.toml" => pyproject(text),
        "requirements.txt" => Some(requirements(text)),
        _ => None,
    };
    parsed.ok_or_else(|| format!("{file} could not be read"))
}

pub fn outdated_command(file: &str) -> Option<&'static [&'static str]> {
    match file {
        "Cargo.toml" => Some(&["cargo", "outdated"]),
        "package.json" => Some(&["npm", "outdated"]),
        "composer.json" => Some(&["composer", "outdated", "--direct"]),
        "go.mod" => Some(&["go", "list", "-u", "-m", "all"]),
        "pyproject.toml" | "requirements.txt" => Some(&["pip", "list", "--outdated"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(file: &str, text: &str) -> Vec<(String, String, String, usize)> {
        parse(file, text)
            .unwrap()
            .into_iter()
            .map(|d| (d.name, d.version, d.section, d.line))
            .collect()
    }

    fn row(
        name: &str,
        version: &str,
        section: &str,
        line: usize,
    ) -> (String, String, String, usize) {
        (
            name.to_owned(),
            version.to_owned(),
            section.to_owned(),
            line,
        )
    }

    #[test]
    fn cargo_in_all_its_shapes() {
        let text = "[package]\nname = \"a\"\n[dependencies]\nserde = \"1\"\nrope = { version = \"2\" }\nlocal = { path = \"../l\" }\n[dev-dependencies]\nserde = \"9\"\n";
        assert_eq!(
            found("Cargo.toml", text),
            [
                row("local", "path", "dependencies", 5),
                row("rope", "2", "dependencies", 4),
                row("serde", "1", "dependencies", 3),
                row("serde", "9", "dev-dependencies", 7),
            ]
        );
    }

    #[test]
    fn node_and_composer() {
        let text = "{\n \"dependencies\": {\n  \"a\": \"^1\"\n },\n \"devDependencies\": {\n  \"b\": \"2\"\n }\n}\n";
        assert_eq!(
            found("package.json", text),
            [
                row("a", "^1", "dependencies", 2),
                row("b", "2", "devDependencies", 5)
            ]
        );
        let text = "{\"require\": {\"php\": \">=8\"}, \"require-dev\": {\"x/y\": \"1\"}}";
        let got = found("composer.json", text);
        assert_eq!(
            (got[0].0.as_str(), got[1].2.as_str()),
            ("php", "require-dev")
        );
    }

    #[test]
    fn go_and_python() {
        let text = "module m\n\nrequire x.io/a v1.0.0\nrequire (\n\tx.io/b v2.0.0\n\tx.io/c v3.0.0 // indirect\n)\n";
        assert_eq!(
            found("go.mod", text),
            [
                row("x.io/a", "v1.0.0", "require", 2),
                row("x.io/b", "v2.0.0", "require", 4),
                row("x.io/c", "v3.0.0", "indirect", 5),
            ]
        );
        let text =
            "# comment\nflask==2.0\nrequests>=2,<3 ; python_version>'3'\n-r other.txt\nbare\n";
        assert_eq!(
            found("requirements.txt", text),
            [
                row("flask", "==2.0", "requirements", 1),
                row("requests", ">=2,<3", "requirements", 2),
                row("bare", "", "requirements", 4),
            ]
        );
        let text = "[project]\ndependencies = [\n  \"flask>=2\",\n]\n[tool.poetry.dependencies]\nnumpy = \"^1\"\n";
        let got = found("pyproject.toml", text);
        assert_eq!(got[0], row("flask", ">=2", "dependencies", 2));
        assert_eq!(got[1], row("numpy", "^1", "poetry", 5));
    }

    #[test]
    fn a_damaged_manifest_is_said_and_does_not_panic() {
        assert!(parse("Cargo.toml", "not [ toml").is_err());
        assert!(parse("package.json", "{").is_err());
        assert!(parse("unknown", "").is_err());
        assert!(parse("go.mod", "\u{0}garbage").unwrap().is_empty());
    }

    #[test]
    fn every_manifest_has_a_way_to_ask_what_is_old() {
        for file in MANIFESTS {
            assert!(outdated_command(file).is_some(), "{file}");
        }
    }
}
