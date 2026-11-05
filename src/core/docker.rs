use serde_json::Value;

use super::cli::{clean_line, main_complaint, safe_word};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DockerSection {
    #[default]
    Services,
    Containers,
    Images,
}

impl DockerSection {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Services => Self::Containers,
            Self::Containers => Self::Images,
            Self::Images => Self::Services,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerImage {
    pub id: String,
    pub name: String,
    pub size: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerRequest {
    Services,
    Containers,
    Images,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerConfirm {
    pub args: Vec<String>,
    pub in_project: bool,
}

#[derive(Debug, Default)]
pub struct DockerPanel {
    pub section: DockerSection,
    pub services: Vec<ComposeService>,
    pub containers: Vec<DockerContainer>,
    pub images: Vec<DockerImage>,
    pub selected: [usize; 3],
    pub compose_note: Option<String>,
    pub daemon_note: Option<String>,
    pub confirm: Option<DockerConfirm>,
    pub pending: super::cli::CliAsks<DockerRequest>,
}

impl DockerPanel {
    pub fn section_len(&self) -> usize {
        match self.section {
            DockerSection::Services => self.services.len(),
            DockerSection::Containers => self.containers.len(),
            DockerSection::Images => self.images.len(),
        }
    }

    pub fn chosen(&self) -> usize {
        self.selected[self.section.index()]
    }

    pub fn step(&mut self, delta: isize) {
        let last = self.section_len().saturating_sub(1);
        let slot = &mut self.selected[self.section.index()];
        *slot = slot.saturating_add_signed(delta).min(last);
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn text(value: &Value, key: &str) -> String {
    clean_line(value.get(key).and_then(Value::as_str).unwrap_or_default())
}

pub fn parse_services(json: &str) -> Result<Vec<ComposeService>, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|_| "docker compose printed no JSON".to_owned())?;
    let Some(services) = value.get("services").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut rows: Vec<ComposeService> = services
        .iter()
        .filter(|(name, _)| safe_word(name))
        .map(|(name, service)| {
            let image = text(service, "image");
            ComposeService {
                name: name.clone(),
                image: if image.is_empty() {
                    "(built)".to_owned()
                } else {
                    image
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

fn json_lines(lines: &str) -> impl Iterator<Item = Value> + '_ {
    lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

fn is_id(id: &str) -> bool {
    safe_word(id) && !id.contains('/')
}

pub fn parse_containers(lines: &str) -> Vec<DockerContainer> {
    json_lines(lines)
        .filter(|row| is_id(&text(row, "ID")))
        .map(|row| DockerContainer {
            id: text(&row, "ID"),
            name: text(&row, "Names"),
            image: text(&row, "Image"),
            state: text(&row, "State"),
            status: text(&row, "Status"),
        })
        .collect()
}

pub fn parse_images(lines: &str) -> Vec<DockerImage> {
    json_lines(lines)
        .filter(|row| is_id(&text(row, "ID")))
        .map(|row| DockerImage {
            id: text(&row, "ID"),
            name: format!("{}:{}", text(&row, "Repository"), text(&row, "Tag")),
            size: text(&row, "Size"),
        })
        .collect()
}

pub fn plain_failure(said: &str) -> String {
    if said.contains("Cannot connect to the Docker daemon")
        || said.contains("Is the docker daemon running")
    {
        return "Docker is not running: start it, then press r".to_owned();
    }
    main_complaint(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSE: &str = include_str!("../../tests/fixtures/docker/compose-config.json");

    #[test]
    fn services_keep_names_and_images_and_no_environment() {
        let rows = parse_services(COMPOSE).unwrap();
        assert_eq!(
            rows,
            vec![
                ComposeService {
                    name: "db".into(),
                    image: "(built)".into()
                },
                ComposeService {
                    name: "web".into(),
                    image: "nginx:${TAG:-1}".into()
                },
            ]
        );
        assert!(!format!("{rows:?}").contains("SECRET"));
    }

    #[test]
    fn a_hostile_service_is_left_out_and_a_hostile_image_cleaned() {
        let json = r#"{"services":{"--privileged":{"image":"x"},"a;b":{},
            "ok":{"image":"evil\u001b]0;t\u0007"}}}"#;
        let rows = parse_services(json).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "ok");
        assert!(!rows[0].image.contains('\u{1b}'));
        assert!(parse_services("not json").is_err());
        assert_eq!(parse_services("{}"), Ok(Vec::new()));
    }

    #[test]
    fn containers_and_images_are_read_line_by_line() {
        let ps = concat!(
            r#"{"Command":"\"nginx\"","ID":"abc123","Image":"nginx","Names":"web\u001b[2J","State":"running","Status":"Up 2 minutes"}"#,
            "\ngarbage\n",
            r#"{"ID":"--rm","Names":"bad"}"#,
        );
        let rows = parse_containers(ps);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "abc123");
        assert_eq!(rows[0].state, "running");
        assert!(!rows[0].name.contains('\u{1b}'));
        let images =
            parse_images(r#"{"ID":"f00d","Repository":"nginx","Tag":"latest","Size":"187MB"}"#);
        assert_eq!(images[0].name, "nginx:latest");
    }

    #[test]
    fn a_stopped_daemon_is_said_plainly() {
        let real = "Cannot connect to the Docker daemon at unix:///x/docker.sock. Is the docker daemon running?";
        assert!(plain_failure(real).starts_with("Docker is not running"));
        assert_eq!(plain_failure("other"), "other");
    }
}
