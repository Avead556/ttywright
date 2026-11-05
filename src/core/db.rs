use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DB_ROW_CAP: usize = 5000;
const CELL_CAP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbKind {
    Sqlite,
    Postgres,
    Mysql,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbServer {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEndpoint {
    Sqlite {
        path: PathBuf,
    },
    Postgres {
        server: DbServer,
        password_env: Option<String>,
    },
    Mysql {
        server: DbServer,
        defaults_file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConnection {
    pub name: String,
    pub endpoint: DbEndpoint,
}

impl DbConnection {
    #[must_use]
    pub fn kind(&self) -> DbKind {
        match self.endpoint {
            DbEndpoint::Sqlite { .. } => DbKind::Sqlite,
            DbEndpoint::Postgres { .. } => DbKind::Postgres,
            DbEndpoint::Mysql { .. } => DbKind::Mysql,
        }
    }

    #[must_use]
    pub fn tables_query(&self) -> &'static str {
        match self.kind() {
            DbKind::Sqlite => {
                "SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name"
            }
            DbKind::Postgres => {
                "SELECT table_schema, table_name FROM information_schema.tables \
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                 ORDER BY 1, 2"
            }
            DbKind::Mysql => {
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = DATABASE() ORDER BY 1"
            }
        }
    }

    #[must_use]
    pub fn browse_query(&self, table: &DbTable) -> String {
        let quote = |name: &str| match self.kind() {
            DbKind::Mysql => format!("`{}`", name.replace('`', "``")),
            DbKind::Sqlite | DbKind::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        };
        let name = match &table.schema {
            Some(schema) => format!("{}.{}", quote(schema), quote(&table.name)),
            None => quote(&table.name),
        };
        format!("SELECT * FROM {name} LIMIT {}", DB_ROW_CAP + 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbClient {
    pub programs: &'static [&'static str],
    pub args: Vec<String>,
    pub env: Vec<(&'static str, String)>,
    pub password_env: Option<String>,
}

#[must_use]
pub fn client_command(connection: &DbConnection, read_only: bool) -> DbClient {
    let owned = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    match &connection.endpoint {
        DbEndpoint::Sqlite { path } => {
            let mut args = owned(&[
                "-batch",
                "-bail",
                "-safe",
                "-init",
                "/dev/null",
                "-nullvalue",
                "NULL",
                "-csv",
                "-header",
            ]);
            if read_only {
                args.push("-readonly".to_owned());
            }
            args.push(path.display().to_string());
            DbClient {
                programs: &["sqlite3"],
                args,
                env: Vec::new(),
                password_env: None,
            }
        }
        DbEndpoint::Postgres {
            server,
            password_env,
        } => {
            let mut args = owned(&["-X", "-w", "-q", "--csv", "-P", "null=NULL"]);
            args.extend(owned(&["-v", "ON_ERROR_STOP=1", "-v", "FETCH_COUNT=1000"]));
            let mut flag = |name: &str, value: Option<String>| {
                if let Some(value) = value {
                    args.push(name.to_owned());
                    args.push(value);
                }
            };
            flag("-h", server.host.clone());
            flag("-p", server.port.map(|p| p.to_string()));
            flag("-U", server.user.clone());
            flag("-d", server.database.clone());
            let env = if read_only {
                vec![(
                    "PGOPTIONS",
                    "-c default_transaction_read_only=on".to_owned(),
                )]
            } else {
                Vec::new()
            };
            DbClient {
                programs: &["psql"],
                args,
                env,
                password_env: password_env.clone(),
            }
        }
        DbEndpoint::Mysql {
            server,
            defaults_file,
        } => {
            let mut args = Vec::new();
            if let Some(file) = defaults_file {
                args.push(format!("--defaults-extra-file={}", file.display()));
            }
            args.extend(owned(&[
                "--batch",
                "--quick",
                "--binary-mode",
                "--no-auto-rehash",
                "--local-infile=0",
                "--connect-timeout=15",
            ]));
            if read_only {
                args.push("--init-command=SET SESSION TRANSACTION READ ONLY".to_owned());
            }
            let mut value = |name: &str, value: Option<String>| {
                if let Some(value) = value {
                    args.push(format!("--{name}={value}"));
                }
            };
            value("host", server.host.clone());
            value("port", server.port.map(|p| p.to_string()));
            value("user", server.user.clone());
            value("database", server.database.clone());
            DbClient {
                programs: &["mysql", "mariadb"],
                args,
                env: Vec::new(),
                password_env: None,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Semicolon,
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    range: Range<usize>,
    text: String,
}

#[derive(Debug, Default)]
struct Lexed {
    tokens: Vec<Token>,
    doubt: bool,
}

fn quoted_end(chars: &[char], start: usize, quote: char) -> Option<usize> {
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn dollar_quote_end(chars: &[char], start: usize) -> Option<Option<usize>> {
    let mut tag_end = start + 1;
    while chars
        .get(tag_end)
        .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    {
        tag_end += 1;
    }
    if chars.get(tag_end) != Some(&'$') {
        return None;
    }
    let tag = &chars[start..=tag_end];
    let found = (tag_end + 1..chars.len())
        .find(|i| chars[*i..].starts_with(tag))
        .map(|i| i + tag.len());
    Some(found)
}

fn lex(kind: DbKind, sql: &str) -> Lexed {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Lexed::default();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && next == Some('-') {
            let spaced = chars.get(i + 2).is_none_or(|c| c.is_whitespace());
            out.doubt |= kind == DbKind::Mysql && !spaced;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '#' && kind == DbKind::Mysql {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            out.doubt |= matches!(chars.get(i + 2), Some('!' | 'M'));
            let end = (i + 2..chars.len().saturating_sub(1))
                .find(|j| chars[*j] == '*' && chars[*j + 1] == '/');
            let Some(end) = end else {
                out.doubt = true;
                break;
            };
            out.doubt |= (i + 2..end).any(|j| chars[j] == '/' && chars[j + 1] == '*');
            i = end + 2;
        } else if c == '\'' || c == '"' || (c == '`' && kind != DbKind::Postgres) {
            let Some(end) = quoted_end(&chars, i, c) else {
                out.doubt = true;
                out.tokens.push(Token {
                    kind: TokenKind::Other,
                    range: i..chars.len(),
                    text: String::new(),
                });
                break;
            };
            out.tokens.push(Token {
                kind: TokenKind::Other,
                range: i..end,
                text: String::new(),
            });
            i = end;
        } else if c == '$'
            && kind == DbKind::Postgres
            && let Some(end) = dollar_quote_end(&chars, i)
        {
            let Some(end) = end else {
                out.doubt = true;
                out.tokens.push(Token {
                    kind: TokenKind::Other,
                    range: i..chars.len(),
                    text: String::new(),
                });
                break;
            };
            out.tokens.push(Token {
                kind: TokenKind::Other,
                range: i..end,
                text: String::new(),
            });
            i = end;
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            {
                i += 1;
            }
            out.tokens.push(Token {
                kind: TokenKind::Word,
                range: start..i,
                text: chars[start..i].iter().collect::<String>().to_lowercase(),
            });
        } else {
            out.tokens.push(Token {
                kind: if c == ';' {
                    TokenKind::Semicolon
                } else {
                    TokenKind::Other
                },
                range: i..i + 1,
                text: c.to_string(),
            });
            i += 1;
        }
    }
    out
}

#[must_use]
pub fn statement_at(kind: DbKind, sql: &str, cursor: usize) -> Option<Range<usize>> {
    let tokens = lex(kind, sql).tokens;
    let mut statements: Vec<Range<usize>> = Vec::new();
    let mut open: Option<Range<usize>> = None;
    let mut ends: Vec<usize> = Vec::new();
    for token in &tokens {
        if token.kind == TokenKind::Semicolon {
            if let Some(statement) = open.take() {
                statements.push(statement);
                ends.push(token.range.end);
            }
        } else {
            let range = open.get_or_insert(token.range.clone());
            range.end = token.range.end;
        }
    }
    if let Some(statement) = open {
        ends.push(usize::MAX);
        statements.push(statement);
    }
    let mut after = None;
    for (statement, end) in statements.iter().zip(&ends) {
        if cursor <= *end {
            return Some(statement.clone());
        }
        after = Some(statement.clone());
    }
    after
}

#[must_use]
pub fn normalized(kind: DbKind, sql: &str) -> Option<String> {
    let tokens = lex(kind, sql).tokens;
    let first = tokens.iter().find(|t| t.kind != TokenKind::Semicolon)?;
    let last = tokens.iter().rfind(|t| t.kind != TokenKind::Semicolon)?;
    let chars: Vec<char> = sql.chars().collect();
    Some(chars[first.range.start..last.range.end].iter().collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Safety {
    ReadOnly,
    Write,
    Refused(String),
}

const READING: &[&str] = &[
    "select", "explain", "show", "with", "describe", "desc", "values", "table",
];
const WRITING: &[&str] = &[
    "insert",
    "update",
    "delete",
    "merge",
    "replace",
    "upsert",
    "drop",
    "create",
    "alter",
    "truncate",
    "grant",
    "revoke",
    "call",
    "do",
    "exec",
    "execute",
    "copy",
    "attach",
    "detach",
    "vacuum",
    "reindex",
    "lock",
    "set",
    "into",
    "outfile",
    "dumpfile",
    "load",
    "rename",
    "pragma",
    "analyze",
    "analyse",
    "begin",
    "start",
    "commit",
    "rollback",
    "prepare",
    "delimiter",
    "system",
    "source",
    "handler",
    "kill",
    "refresh",
    "cluster",
    "notify",
    "listen",
];
const NOT_CALLS: &[&str] = &[
    "in",
    "from",
    "join",
    "on",
    "and",
    "or",
    "not",
    "where",
    "select",
    "as",
    "over",
    "values",
    "exists",
    "by",
    "when",
    "then",
    "else",
    "using",
    "union",
    "having",
    "case",
    "all",
    "any",
    "some",
    "with",
    "distinct",
    "partition",
    "filter",
    "between",
    "like",
    "intersect",
    "except",
    "limit",
    "offset",
    "materialized",
    "lateral",
];
const PURE_CALLS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "coalesce",
    "nullif",
    "ifnull",
    "lower",
    "upper",
    "length",
    "char_length",
    "trim",
    "ltrim",
    "rtrim",
    "substr",
    "substring",
    "round",
    "abs",
    "floor",
    "ceil",
    "ceiling",
    "cast",
    "concat",
    "date",
    "datetime",
    "strftime",
    "date_trunc",
    "extract",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "to_char",
    "group_concat",
    "string_agg",
    "array_agg",
    "typeof",
];

#[must_use]
pub fn classify(kind: DbKind, sql: &str) -> Safety {
    match kind {
        DbKind::Postgres if sql.contains('\\') => {
            return Safety::Refused(
                "psql would treat a backslash as its own command, which can run programs: \
                 use Terminal for this statement"
                    .to_owned(),
            );
        }
        DbKind::Sqlite if sql.lines().any(|l| l.trim_start().starts_with('.')) => {
            return Safety::Refused(
                "A line starting with a dot is a sqlite3 command: use Terminal for it".to_owned(),
            );
        }
        _ => {}
    }
    let lexed = lex(kind, sql);
    if lexed.doubt || (kind == DbKind::Mysql && sql.contains('\\')) {
        return Safety::Write;
    }
    let tokens = &lexed.tokens;
    let Some(first) = tokens.first() else {
        return Safety::Write;
    };
    if first.kind != TokenKind::Word || !READING.contains(&first.text.as_str()) {
        return Safety::Write;
    }
    let meaningful = tokens.iter().rposition(|t| t.kind != TokenKind::Semicolon);
    let more = tokens
        .iter()
        .enumerate()
        .any(|(i, t)| t.kind == TokenKind::Semicolon && meaningful.is_some_and(|last| i < last));
    if more {
        return Safety::Write;
    }
    for (i, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Word {
            continue;
        }
        let word = token.text.as_str();
        if WRITING.contains(&word) {
            return Safety::Write;
        }
        let called = tokens.get(i + 1).is_some_and(|n| n.text == "(");
        if called && !NOT_CALLS.contains(&word) && !PURE_CALLS.contains(&word) {
            return Safety::Write;
        }
    }
    Safety::ReadOnly
}

#[must_use]
pub fn is_unsafe_on_screen(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}')
}

pub fn clean_cell(text: &str) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i == CELL_CAP {
            out.push('…');
            break;
        }
        match c {
            '\n' => out.push('↵'),
            '\r' => {}
            '\t' => out.push(' '),
            c if is_unsafe_on_screen(c) => out.push('�'),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbGrid {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

fn into_grid(mut records: Vec<Vec<String>>, cut: bool, elapsed_ms: u64) -> DbGrid {
    if records.is_empty() {
        return DbGrid {
            truncated: cut,
            elapsed_ms,
            ..DbGrid::default()
        };
    }
    let columns = records.remove(0);
    let truncated = cut || records.len() > DB_ROW_CAP;
    records.truncate(DB_ROW_CAP);
    DbGrid {
        columns,
        rows: records,
        truncated,
        elapsed_ms,
    }
}

#[must_use]
pub fn parse_csv(bytes: &[u8], cut: bool, elapsed_ms: u64) -> DbGrid {
    let text = String::from_utf8_lossy(bytes);
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut started = false;
    while let Some(c) = chars.next() {
        started = true;
        if quoted {
            if c != '"' {
                cell.push(c);
            } else if chars.peek() == Some(&'"') {
                chars.next();
                cell.push('"');
            } else {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            ',' => record.push(clean_cell(&std::mem::take(&mut cell))),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                record.push(clean_cell(&std::mem::take(&mut cell)));
                records.push(std::mem::take(&mut record));
                started = false;
            }
            c => cell.push(c),
        }
    }
    if started && !cut {
        record.push(clean_cell(&cell));
        records.push(record);
    }
    into_grid(records, cut, elapsed_ms)
}

#[must_use]
pub fn parse_tsv(bytes: &[u8], cut: bool, elapsed_ms: u64) -> DbGrid {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.pop();
    let records = lines
        .into_iter()
        .map(|line| {
            line.split('\t')
                .map(|cell| clean_cell(&unescape_batch(cell)))
                .collect()
        })
        .collect();
    into_grid(records, cut, elapsed_ms)
}

fn unescape_batch(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut chars = cell.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbOutcome {
    Done(DbGrid),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTable {
    pub schema: Option<String>,
    pub name: String,
}

impl DbTable {
    #[must_use]
    pub fn label(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[must_use]
pub fn tables_from(grid: &DbGrid) -> Vec<DbTable> {
    grid.rows
        .iter()
        .filter_map(|row| match row.as_slice() {
            [name] => Some(DbTable {
                schema: None,
                name: name.clone(),
            }),
            [schema, name] => Some(DbTable {
                schema: Some(schema.clone()),
                name: name.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DbSection {
    #[default]
    Connections,
    Tables,
    Results,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbPurpose {
    ListTables,
    Grid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbConfirm {
    pub connection: usize,
    pub statement: String,
    pub scroll: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbPanel {
    pub connections: Vec<DbConnection>,
    pub section: DbSection,
    pub selected_connection: usize,
    pub active: Option<usize>,
    pub tables: Vec<DbTable>,
    pub selected_table: usize,
    pub running: Option<(u64, DbPurpose)>,
    pub title: String,
    pub outcome: Option<DbOutcome>,
    pub row: usize,
    pub column: usize,
    pub confirm: Option<DbConfirm>,
}

impl DbPanel {
    pub fn rows_shown(&self) -> usize {
        match &self.outcome {
            Some(DbOutcome::Done(grid)) => grid.rows.len(),
            _ => 0,
        }
    }

    pub fn columns_shown(&self) -> usize {
        match &self.outcome {
            Some(DbOutcome::Done(grid)) => grid.columns.len(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEffect {
    Query {
        id: u64,
        connection: DbConnection,
        statement: String,
        read_only: bool,
    },
    Cancel {
        id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEvent {
    Finished { id: u64, outcome: DbOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbState {
    panel: DbPanel,
    next_id: u64,
}

impl Default for DbState {
    fn default() -> Self {
        Self {
            panel: DbPanel::default(),
            next_id: 1,
        }
    }
}

impl DbState {
    pub const fn panel(&self) -> &DbPanel {
        &self.panel
    }

    pub fn configure(&mut self, connections: Vec<DbConnection>) {
        self.panel.connections = connections;
    }

    pub fn connection(&self) -> Option<usize> {
        self.panel
            .active
            .or_else(|| (self.panel.connections.len() == 1).then_some(0))
    }

    pub fn send(
        &mut self,
        index: usize,
        statement: String,
        read_only: bool,
        purpose: DbPurpose,
    ) -> Vec<DbEffect> {
        let Some(connection) = self.panel.connections.get(index).cloned() else {
            return Vec::new();
        };
        let mut fx = Vec::new();
        if let Some((id, _)) = self.panel.running.take() {
            fx.push(DbEffect::Cancel { id });
        }
        let id = self.next_id;
        self.next_id += 1;
        let panel = &mut self.panel;
        panel.active = Some(index);
        panel.running = Some((id, purpose));
        if purpose == DbPurpose::Grid {
            panel.title = clean_cell(&statement);
            panel.outcome = None;
            panel.row = 0;
            panel.column = 0;
        }
        fx.push(DbEffect::Query {
            id,
            connection,
            statement,
            read_only,
        });
        fx
    }

    pub const fn stop(&self) -> Option<DbEffect> {
        match self.panel.running {
            Some((id, _)) => Some(DbEffect::Cancel { id }),
            None => None,
        }
    }

    pub fn handle(&mut self, event: DbEvent) {
        let DbEvent::Finished { id, outcome } = event;
        let panel = &mut self.panel;
        let Some((running, purpose)) = panel.running else {
            return;
        };
        if running != id {
            return;
        }
        panel.running = None;
        match (purpose, outcome) {
            (DbPurpose::ListTables, DbOutcome::Done(grid)) => {
                panel.tables = tables_from(&grid);
                panel.selected_table = 0;
                panel.section = DbSection::Tables;
            }
            (_, outcome) => {
                panel.outcome = Some(outcome);
                panel.row = 0;
                panel.column = 0;
            }
        }
    }

    pub fn ask(&mut self, connection: usize, statement: String) {
        self.panel.confirm = Some(DbConfirm {
            connection,
            statement,
            scroll: 0,
        });
    }

    pub const fn asking(&self) -> bool {
        self.panel.confirm.is_some()
    }

    pub fn confirm(&mut self) -> Vec<DbEffect> {
        match self.panel.confirm.take() {
            Some(c) => self.send(c.connection, c.statement, false, DbPurpose::Grid),
            None => Vec::new(),
        }
    }

    pub fn decline(&mut self) {
        self.panel.confirm = None;
    }

    pub fn scroll_confirm(&mut self, delta: isize) {
        if let Some(confirm) = self.panel.confirm.as_mut() {
            confirm.scroll = confirm.scroll.saturating_add_signed(delta);
        }
    }

    pub fn pick(&mut self, table: bool, index: usize) -> Vec<DbEffect> {
        let panel = &mut self.panel;
        if table {
            panel.section = DbSection::Tables;
            if index < panel.tables.len() {
                panel.selected_table = index;
                return self.open_table();
            }
        } else {
            panel.section = DbSection::Connections;
            if index < panel.connections.len() {
                panel.selected_connection = index;
                return self.open_connection();
            }
        }
        Vec::new()
    }

    pub fn enter(&mut self) -> Vec<DbEffect> {
        match self.panel.section {
            DbSection::Connections => self.open_connection(),
            DbSection::Tables => self.open_table(),
            DbSection::Results => Vec::new(),
        }
    }

    fn open_connection(&mut self) -> Vec<DbEffect> {
        let index = self.panel.selected_connection;
        let Some(connection) = self.panel.connections.get(index) else {
            return Vec::new();
        };
        let query = connection.tables_query().to_owned();
        self.panel.tables.clear();
        self.panel.selected_table = 0;
        self.send(index, query, true, DbPurpose::ListTables)
    }

    fn open_table(&mut self) -> Vec<DbEffect> {
        let Some(index) = self.panel.active else {
            return Vec::new();
        };
        let (Some(connection), Some(table)) = (
            self.panel.connections.get(index),
            self.panel.tables.get(self.panel.selected_table),
        ) else {
            return Vec::new();
        };
        let query = connection.browse_query(table);
        self.panel.section = DbSection::Results;
        self.send(index, query, true, DbPurpose::Grid)
    }

    pub fn scroll(&mut self, delta: isize) {
        let last = self.panel.rows_shown().saturating_sub(1);
        self.panel.row = self.panel.row.saturating_add_signed(delta).min(last);
    }

    pub fn step(&mut self, delta: isize) {
        let step = |value: usize, len: usize| {
            value
                .saturating_add_signed(delta)
                .min(len.saturating_sub(1))
        };
        let panel = &mut self.panel;
        match panel.section {
            DbSection::Connections => {
                panel.selected_connection =
                    step(panel.selected_connection, panel.connections.len());
            }
            DbSection::Tables => {
                panel.selected_table = step(panel.selected_table, panel.tables.len());
            }
            DbSection::Results => self.scroll(delta),
        }
    }

    pub fn next_section(&mut self) {
        self.panel.section = match self.panel.section {
            DbSection::Connections => DbSection::Tables,
            DbSection::Tables => DbSection::Results,
            DbSection::Results => DbSection::Connections,
        };
    }

    pub fn step_column(&mut self, delta: isize) {
        if self.panel.section != DbSection::Results {
            return;
        }
        let last = self.panel.columns_shown().saturating_sub(1);
        self.panel.column = self.panel.column.saturating_add_signed(delta).min(last);
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> DbOutcome {
        DbOutcome::Done(DbGrid {
            columns: vec!["a".to_owned(), "b".to_owned()],
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
                .collect(),
            ..DbGrid::default()
        })
    }

    fn query_id(fx: &[DbEffect]) -> u64 {
        fx.iter()
            .find_map(|e| match e {
                DbEffect::Query { id, .. } => Some(*id),
                DbEffect::Cancel { .. } => None,
            })
            .unwrap()
    }

    fn state() -> DbState {
        let mut db = DbState::default();
        db.configure(vec![DbConnection {
            name: "a".to_owned(),
            endpoint: DbEndpoint::Sqlite {
                path: "a.db".into(),
            },
        }]);
        db
    }

    #[test]
    fn a_second_statement_cancels_the_first_and_drops_its_answer() {
        let mut db = state();
        let first = query_id(&db.send(0, "select 1".to_owned(), true, DbPurpose::Grid));
        let fx = db.send(0, "select 2".to_owned(), true, DbPurpose::Grid);
        assert_eq!(fx[0], DbEffect::Cancel { id: first });
        let second = query_id(&fx);
        db.handle(DbEvent::Finished {
            id: first,
            outcome: grid(&[]),
        });
        assert_eq!(db.panel().outcome, None);
        db.handle(DbEvent::Finished {
            id: second,
            outcome: grid(&[&["1", "2"]]),
        });
        assert_eq!(db.panel().running, None);
        assert_eq!(db.panel().outcome, Some(grid(&[&["1", "2"]])));
        assert_eq!(db.stop(), None);
    }

    #[test]
    fn listing_tables_fills_the_tables_and_not_the_grid() {
        let mut db = state();
        let id = query_id(&db.pick(false, 0));
        db.handle(DbEvent::Finished {
            id,
            outcome: grid(&[&["main", "users"]]),
        });
        assert_eq!(db.panel().tables[0].label(), "main.users");
        assert_eq!(db.panel().section, DbSection::Tables);
        assert_eq!(db.panel().outcome, None);
        assert!(matches!(
            db.enter()[..],
            [DbEffect::Query {
                read_only: true,
                ..
            }]
        ));
        assert_eq!(db.panel().section, DbSection::Results);
    }

    #[test]
    fn a_write_runs_only_after_yes() {
        let mut db = state();
        db.ask(0, "delete from t".to_owned());
        assert!(db.asking());
        db.decline();
        assert!(!db.asking());
        assert!(db.confirm().is_empty(), "nothing left to run");
        db.ask(0, "delete from t".to_owned());
        assert!(matches!(
            db.confirm()[..],
            [DbEffect::Query {
                read_only: false,
                ..
            }]
        ));
    }

    #[test]
    fn moving_stays_within_what_is_shown() {
        let mut db = state();
        let id = query_id(&db.send(0, "select".to_owned(), true, DbPurpose::Grid));
        db.handle(DbEvent::Finished {
            id,
            outcome: grid(&[&["1", "2"], &["3", "4"]]),
        });
        db.step_column(5);
        assert_eq!(db.panel().column, 0, "not in the results section");
        db.next_section();
        db.next_section();
        db.step(10);
        db.step_column(5);
        assert_eq!((db.panel().row, db.panel().column), (1, 1));
        db.step(-10);
        assert_eq!(db.panel().row, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DbKind::{Mysql, Postgres, Sqlite};

    #[test]
    fn plain_reads_are_reads() {
        for kind in [Sqlite, Postgres, Mysql] {
            for sql in [
                "SELECT * FROM t",
                "select a, count(*) from t where b in (1, 2) group by a;",
                "WITH x AS (SELECT 1) SELECT * FROM x",
                "EXPLAIN SELECT 1",
                "SHOW TABLES",
                "SELECT 'DROP TABLE x; insert' FROM t",
                "-- a comment: drop table x\nSELECT 1",
                "SELECT 1 /* delete */ FROM t ;  -- the end",
            ] {
                assert_eq!(classify(kind, sql), Safety::ReadOnly, "{kind:?}: {sql}");
            }
        }
    }

    #[test]
    fn anything_else_or_in_doubt_counts_as_a_write() {
        for kind in [Sqlite, Postgres, Mysql] {
            for sql in [
                "DROP TABLE t",
                "delete from t",
                "UPDATE t SET a = 1",
                "SELECT 1; DROP TABLE t",
                "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
                "SELECT * INTO copy FROM t",
                "SELECT * FROM t FOR UPDATE",
                "EXPLAIN ANALYZE DELETE FROM t",
                "SELECT pg_terminate_backend(1)",
                "SELECT sleep (100)",
                "SELECT load_extension('x')",
                "SELECT * INTO OUTFILE '/tmp/x' FROM t",
                "PRAGMA writable_schema = 1",
                "SELECT 'unterminated",
                "SELECT 1 /* unterminated",
                "SELECT 1 /* a /* nested */ ; DROP TABLE t; /* */",
                "",
                "-- only a comment",
            ] {
                assert_eq!(classify(kind, sql), Safety::Write, "{kind:?}: {sql}");
            }
        }
    }

    #[test]
    fn what_mysql_reads_differently_is_in_doubt() {
        for sql in [
            "SELECT 1 /*! ; DROP TABLE t */",
            "SELECT 1 /*M! ; DROP TABLE t */",
            r"SELECT '\'' ; DROP TABLE t; -- '",
            "SELECT 1--1 ; DROP TABLE t",
        ] {
            assert_eq!(classify(Mysql, sql), Safety::Write, "{sql}");
        }
        assert_eq!(classify(Mysql, "SELECT 1 # ; \nFROM t"), Safety::ReadOnly);
    }

    #[test]
    fn what_the_client_would_obey_itself_is_refused() {
        for sql in [
            r"SELECT 1 \! rm -rf /",
            r"\! rm -rf /",
            r"SELECT E'\'' \! x --'",
        ] {
            assert!(
                matches!(classify(Postgres, sql), Safety::Refused(_)),
                "{sql}"
            );
        }
        for sql in [".shell rm -rf /", "SELECT 1;\n  .system x"] {
            assert!(matches!(classify(Sqlite, sql), Safety::Refused(_)), "{sql}");
        }
    }

    #[test]
    fn the_statement_at_the_cursor_ignores_semicolons_in_strings_and_comments() {
        let sql = "SELECT ';' ; -- a;b\nSELECT 2;\n\nSELECT 3";
        let at = |cursor| {
            let range = statement_at(Sqlite, sql, cursor).unwrap();
            sql.chars()
                .skip(range.start)
                .take(range.len())
                .collect::<String>()
        };
        assert_eq!(at(0), "SELECT ';'");
        assert_eq!(at(12), "SELECT ';'", "just after its semicolon");
        assert_eq!(at(25), "SELECT 2");
        assert_eq!(at(sql.chars().count()), "SELECT 3");
        assert_eq!(statement_at(Sqlite, "  -- nothing\n", 0), None);
    }

    #[test]
    fn after_the_last_semicolon_the_last_statement_is_meant() {
        let sql = "SELECT 1;\n\n";
        let range = statement_at(Sqlite, sql, sql.len()).unwrap();
        assert_eq!(range, 0..8);
    }

    #[test]
    fn dollar_quotes_belong_to_postgres_only() {
        let sql = "SELECT $x$ ; $x$; SELECT 2";
        assert_eq!(statement_at(Postgres, sql, 0), Some(0..16));
        assert_eq!(statement_at(Sqlite, sql, 0), Some(0..10));
    }

    #[test]
    fn what_is_sent_has_no_last_semicolon_or_surrounding_comments() {
        assert_eq!(
            normalized(Sqlite, "-- c\nSELECT 1 ; -- d\n").as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(normalized(Sqlite, " ; -- c"), None);
    }

    #[test]
    fn csv_keeps_quotes_commas_breaks_and_cleans_values() {
        let grid = parse_csv(
            b"a,b\r\n\"x,1\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",\x1b[31mred\r\n",
            false,
            7,
        );
        assert_eq!(grid.columns, ["a", "b"]);
        assert_eq!(grid.rows[0], ["x,1", "say \"hi\""]);
        assert_eq!(grid.rows[1], ["two↵lines", "�[31mred"]);
        assert_eq!(grid.elapsed_ms, 7);
        assert_eq!(parse_csv(b"", false, 0), DbGrid::default());
    }

    #[test]
    fn a_cut_answer_drops_its_partial_row_and_says_so() {
        let grid = parse_csv(b"a\n1\n2", true, 0);
        assert_eq!(grid.rows, [["1"]]);
        assert!(grid.truncated);
    }

    #[test]
    fn rows_past_the_cap_are_not_kept() {
        let mut text = String::from("a\n");
        for n in 0..=DB_ROW_CAP {
            text.push_str(&format!("{n}\n"));
        }
        let grid = parse_csv(text.as_bytes(), false, 0);
        assert_eq!(grid.rows.len(), DB_ROW_CAP);
        assert!(grid.truncated);
    }

    #[test]
    fn batch_output_is_unescaped_and_cleaned() {
        let grid = parse_tsv(b"a\tb\nx\\ny\t\\\\\n\x07\tNULL\n", false, 0);
        assert_eq!(grid.columns, ["a", "b"]);
        assert_eq!(grid.rows[0], ["x↵y", "\\"]);
        assert_eq!(grid.rows[1], ["�", "NULL"]);
    }

    #[test]
    fn hidden_and_reordering_characters_do_not_reach_the_screen() {
        assert_eq!(clean_cell("a\u{202E}b\u{200B}c\u{9b}"), "a�b�c�");
        assert_eq!(
            clean_cell(&"x".repeat(CELL_CAP + 1)).chars().last(),
            Some('…')
        );
    }

    #[test]
    fn a_table_name_cannot_break_out_of_its_quotes() {
        let table = DbTable {
            schema: None,
            name: "a\"; DROP TABLE b; --".to_owned(),
        };
        let sqlite = DbConnection {
            name: "x".to_owned(),
            endpoint: DbEndpoint::Sqlite { path: "/x".into() },
        };
        let query = sqlite.browse_query(&table);
        assert!(query.contains("\"a\"\"; DROP TABLE b; --\""), "{query}");
        assert_eq!(classify(Sqlite, &query), Safety::ReadOnly);
        let mysql = DbConnection {
            name: "x".to_owned(),
            endpoint: DbEndpoint::Mysql {
                server: DbServer::default(),
                defaults_file: None,
            },
        };
        let query = mysql.browse_query(&DbTable {
            schema: None,
            name: "a`b".to_owned(),
        });
        assert!(query.contains("`a``b`"), "{query}");
    }

    #[test]
    fn clients_get_their_guards_and_reads_are_enforced_by_them() {
        let sqlite = DbConnection {
            name: "s".to_owned(),
            endpoint: DbEndpoint::Sqlite { path: "/x".into() },
        };
        let client = client_command(&sqlite, true);
        assert!(client.args.iter().any(|a| a == "-safe"));
        assert!(client.args.iter().any(|a| a == "-readonly"));
        assert!(
            !client_command(&sqlite, false)
                .args
                .iter()
                .any(|a| a == "-readonly")
        );

        let pg = DbConnection {
            name: "p".to_owned(),
            endpoint: DbEndpoint::Postgres {
                server: DbServer {
                    host: Some("h".to_owned()),
                    ..DbServer::default()
                },
                password_env: Some("PW".to_owned()),
            },
        };
        let client = client_command(&pg, true);
        assert!(client.args.iter().any(|a| a == "-X"));
        assert!(client.args.iter().any(|a| a == "-w"));
        assert_eq!(client.password_env.as_deref(), Some("PW"));
        assert!(client.env[0].1.contains("read_only=on"));
        assert!(client_command(&pg, false).env.is_empty());

        let my = DbConnection {
            name: "m".to_owned(),
            endpoint: DbEndpoint::Mysql {
                server: DbServer::default(),
                defaults_file: Some("/c.cnf".into()),
            },
        };
        let client = client_command(&my, true);
        assert_eq!(client.args[0], "--defaults-extra-file=/c.cnf");
        for wanted in ["--binary-mode", "--local-infile=0", "--batch"] {
            assert!(client.args.iter().any(|a| a == wanted), "{wanted}");
        }
        assert!(client.args.iter().any(|a| a.contains("READ ONLY")));
    }

    #[test]
    fn tables_come_with_or_without_a_schema() {
        let grid = DbGrid {
            rows: vec![
                vec!["t".to_owned()],
                vec!["public".to_owned(), "u".to_owned()],
            ],
            ..DbGrid::default()
        };
        let tables = tables_from(&grid);
        assert_eq!(tables[0].label(), "t");
        assert_eq!(tables[1].label(), "public.u");
    }
}
