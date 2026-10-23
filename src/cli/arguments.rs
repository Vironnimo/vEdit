//! Token ownership, global flags and command option constraints.
use super::*;

pub fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    let mut args: Vec<String> = arguments
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| "Arguments must be valid Unicode".to_owned())
        })
        .collect::<Result<_, _>>()?;
    if args.is_empty() {
        return Ok(Parsed::Help { command: None });
    }
    args = normalize_equals(args);
    // A label or other option value may literally be `--help`, `--`, or
    // `--no-start`. Only interpret global flags in argument positions owned
    // by the CLI, never inside a supplied value.
    let mut literal = false;
    let mut expects_value = false;
    let mut help = false;
    let mut no_start = false;
    args.retain(|arg| {
        if literal {
            return true;
        }
        if expects_value {
            expects_value = false;
            return true;
        }
        match arg.as_str() {
            "--" => literal = true,
            "--help" | "-h" => help = true,
            "--no-start" => {
                no_start = true;
                return false;
            }
            value if help::takes_value(value) => expects_value = true,
            _ => {}
        }
        true
    });
    if help {
        let command = args
            .first()
            .filter(|arg| !matches!(arg.as_str(), "--help" | "-h"))
            .cloned();
        if let Some(name) = &command
            && help::command(name).is_none()
        {
            return Err(format!("Unknown command {name}; use vedit-cli --help"));
        }
        return Ok(Parsed::Help { command });
    }
    if args.is_empty() {
        return Err("A command is required after --no-start; use --help".into());
    }
    let command = args.remove(0);
    if command == "windows" {
        if !args.is_empty() {
            return Err("windows takes no arguments; use windows --help".into());
        }
        return Ok(Parsed::Windows);
    }
    if command == "schema" {
        if args.len() > 1 {
            return Err("schema takes at most one operation name; use schema --help".into());
        }
        let topic = args.first().cloned();
        schema_topic(topic.as_deref())?;
        return Ok(Parsed::Schema { topic });
    }
    if help::command(&command).is_none() {
        return Err(format!("Unknown command {command}; use vedit-cli --help"));
    }
    let mut options = Options::default();
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => set_option(
                &mut options.workspace,
                iter.next().ok_or("--workspace needs a project path")?,
                "--workspace",
            )?,
            "--" => {
                positional.extend(iter);
                break;
            }
            "--line" => set_option(&mut options.line, number(iter.next(), "--line")?, "--line")?,
            "--column" => set_option(
                &mut options.column,
                number(iter.next(), "--column")?,
                "--column",
            )?,
            "--limit" => set_option(
                &mut options.limit,
                number(iter.next(), "--limit")?,
                "--limit",
            )?,
            "--offset" => set_option(
                &mut options.offset,
                number(iter.next(), "--offset")?,
                "--offset",
            )?,
            "--count" => set_option(
                &mut options.count,
                number(iter.next(), "--count")?,
                "--count",
            )?,
            "--revision" => set_option(
                &mut options.revision,
                iter.next()
                    .ok_or("--revision needs a value")?
                    .parse::<u64>()
                    .map_err(|_| "--revision needs a nonnegative integer")?,
                "--revision",
            )?,
            "--tab" => set_option(
                &mut options.tab,
                tab_id(&iter.next().ok_or("--tab needs a tab ID")?)?,
                "--tab",
            )?,
            "--file" => set_option(
                &mut options.file,
                iter.next().ok_or("--file needs a text file path")?,
                "--file",
            )?,
            "--path" => set_option(
                &mut options.path,
                iter.next().ok_or("--path needs a destination path")?,
                "--path",
            )?,
            "--label" => set_option(
                &mut options.label,
                iter.next().ok_or("--label needs text")?,
                "--label",
            )?,
            "--note" => set_option(
                &mut options.note,
                iter.next().ok_or("--note needs text")?,
                "--note",
            )?,
            "--batch" => set_option(
                &mut options.batch,
                agent_mark_id(&iter.next().ok_or("--batch needs a batch ID")?)?,
                "--batch",
            )?,
            "--request-id" => set_option(
                &mut options.request_id,
                iter.next().ok_or("--request-id needs a token")?,
                "--request-id",
            )?,
            "--exclude" => {
                if options.no_excludes {
                    return Err("--exclude and --no-excludes are contradictory".into());
                }
                options.excludes.get_or_insert_with(Vec::new).push(
                    iter.next()
                        .ok_or("--exclude needs a folder or glob pattern")?,
                );
            }
            "--no-excludes" => {
                if options.excludes.is_some() {
                    return Err("--exclude and --no-excludes are contradictory".into());
                }
                set_flag(&mut options.no_excludes, "--no-excludes")?;
            }
            "--working" => set_flag(&mut options.working, "--working")?,
            "--background" => set_flag(&mut options.background, "--background")?,
            "--title" => set_option(
                &mut options.title,
                iter.next().ok_or("--title needs text")?,
                "--title",
            )?,
            "--level" => set_option(
                &mut options.level,
                serde_json::from_value(json!(
                    iter.next()
                        .ok_or("--level needs info, success, warning or error")?
                        .to_ascii_lowercase()
                ))
                .map_err(|_| "--level needs info, success, warning or error")?,
                "--level",
            )?,
            "--open" => set_option(
                &mut options.open,
                iter.next().ok_or("--open needs PATH or PATH:LINE")?,
                "--open",
            )?,
            "--review" => set_option(
                &mut options.review,
                serde_json::from_value(json!(
                    iter.next()
                        .ok_or("--review needs working or branch")?
                        .to_ascii_lowercase()
                ))
                .map_err(|_| "--review needs working or branch")?,
                "--review",
            )?,
            "--branch" => set_flag(&mut options.branch, "--branch")?,
            "--commit" => set_option(
                &mut options.commit,
                iter.next().ok_or("--commit needs a revision")?,
                "--commit",
            )?,
            "--base" => set_option(
                &mut options.base,
                iter.next().ok_or("--base needs a branch or revision")?,
                "--base",
            )?,
            "--case-sensitive" => set_flag(&mut options.case_sensitive, "--case-sensitive")?,
            "--whole-word" => set_flag(&mut options.whole_word, "--whole-word")?,
            "--first" => set_flag(&mut options.first, "--first")?,
            "--preview" => set_option(&mut options.view, DocumentView::Preview, "view")?,
            "--source" => set_option(&mut options.view, DocumentView::Source, "view")?,
            "--hide" => {
                if options.hide {
                    return Err("Duplicate --hide".into());
                }
                options.hide = true;
            }
            "-" if command == "request" => positional.push(arg),
            value if value.starts_with('-') => {
                return Err(format!("Unknown option {value}; use --help"));
            }
            _ => positional.push(arg),
        }
    }
    options.allowed(&command)?;
    let mut request = commands::request(&command, positional, options)?;
    resolve_paths(&mut request);
    validate(&request)?;
    // A background presentation never opens a new window in front of the user.
    let start = !no_start
        && matches!(
            request,
            Request::Open {
                background: false,
                ..
            } | Request::Review {
                background: false,
                ..
            } | Request::Workspace { .. }
        );
    Ok(Parsed::Command { request, start })
}

#[derive(Default)]
pub(super) struct Options {
    pub(super) workspace: Option<String>,
    pub(super) file: Option<String>,
    pub(super) path: Option<String>,
    pub(super) line: Option<usize>,
    pub(super) column: Option<usize>,
    pub(super) view: Option<DocumentView>,
    pub(super) limit: Option<usize>,
    pub(super) offset: Option<usize>,
    pub(super) count: Option<usize>,
    pub(super) revision: Option<u64>,
    pub(super) tab: Option<u64>,
    pub(super) label: Option<String>,
    pub(super) note: Option<String>,
    pub(super) batch: Option<u64>,
    pub(super) request_id: Option<String>,
    pub(super) excludes: Option<Vec<String>>,
    pub(super) no_excludes: bool,
    pub(super) case_sensitive: bool,
    pub(super) whole_word: bool,
    pub(super) first: bool,
    pub(super) hide: bool,
    pub(super) working: bool,
    pub(super) branch: bool,
    pub(super) commit: Option<String>,
    pub(super) base: Option<String>,
    pub(super) background: bool,
    pub(super) title: Option<String>,
    pub(super) level: Option<NotificationLevel>,
    pub(super) open: Option<String>,
    pub(super) review: Option<NotificationReview>,
}
impl Options {
    /// The project from `--workspace` or a legacy positional WORKSPACE, never both.
    pub(super) fn workspace_or(&self, positional: &[String]) -> Result<Option<PathBuf>, String> {
        match (&self.workspace, positional) {
            (Some(_), [_, ..]) => {
                Err("Give the project either as WORKSPACE or with --workspace, not both".into())
            }
            (Some(path), []) => Ok(Some(absolute(path.into()))),
            (None, [path, ..]) => Ok(Some(absolute(path.into()))),
            (None, []) => Ok(None),
        }
    }
    fn allowed(&self, command: &str) -> Result<(), String> {
        let definition = help::command(command)
            .ok_or_else(|| format!("Unknown command {command}; use vedit-cli --help"))?;
        for (present, flag) in [
            (self.workspace.is_some(), "--workspace"),
            (self.line.is_some(), "--line"),
            (self.column.is_some(), "--column"),
            (self.file.is_some(), "--file"),
            (self.path.is_some(), "--path"),
            (self.view.is_some(), "--preview"),
            (self.limit.is_some(), "--limit"),
            (self.offset.is_some(), "--offset"),
            (self.count.is_some(), "--count"),
            (self.revision.is_some(), "--revision"),
            (self.tab.is_some(), "--tab"),
            (self.label.is_some(), "--label"),
            (self.note.is_some(), "--note"),
            (self.batch.is_some(), "--batch"),
            (self.request_id.is_some(), "--request-id"),
            (self.excludes.is_some(), "--exclude"),
            (self.no_excludes, "--no-excludes"),
            (self.case_sensitive, "--case-sensitive"),
            (self.whole_word, "--whole-word"),
            (self.first, "--first"),
            (self.hide, "--hide"),
            (self.working, "--working"),
            (self.branch, "--branch"),
            (self.commit.is_some(), "--commit"),
            (self.base.is_some(), "--base"),
            (self.background, "--background"),
            (self.title.is_some(), "--title"),
            (self.level.is_some(), "--level"),
            (self.open.is_some(), "--open"),
            (self.review.is_some(), "--review"),
        ] {
            if present && !definition.flags.contains(&flag) {
                return Err(format!(
                    "{flag} is not supported by {command}; use vedit-cli {command} --help"
                ));
            }
        }
        if self.column.is_some() && self.line.is_none() {
            return Err("--column requires --line".into());
        }
        Ok(())
    }
    pub(super) fn location(&self) -> Result<Option<Location>, String> {
        if self.column.is_some() && self.line.is_none() {
            return Err("--column requires --line".into());
        }
        Ok(self.line.map(|line| Location {
            line,
            column: self.column.unwrap_or(1),
        }))
    }
    pub(super) fn required_revision(&self) -> Result<u64, String> {
        self.revision.ok_or_else(|| {
            "This command requires --revision N from read/tabs; stale edits are refused".into()
        })
    }
}
fn set_flag(field: &mut bool, name: &str) -> Result<(), String> {
    if *field {
        return Err(format!("Duplicate {name}"));
    }
    *field = true;
    Ok(())
}

fn set_option<T>(field: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if field.is_some() {
        return Err(format!("Duplicate or contradictory {name}"));
    }
    *field = Some(value);
    Ok(())
}
fn number(value: Option<String>, flag: &str) -> Result<usize, String> {
    value
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a nonnegative integer"))
}
pub(super) fn tab_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "Tab ID must be a positive integer returned by tabs/status".into())
}
pub(super) fn ids(values: &[String], required: bool) -> Result<Vec<u64>, String> {
    if required && values.is_empty() {
        return Err("One or more tab IDs are required".into());
    }
    values.iter().map(|v| tab_id(v)).collect()
}
pub(super) fn require_empty(values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        Ok(())
    } else {
        Err("Unexpected positional arguments".into())
    }
}
pub(super) fn one_argument<'a>(values: &'a [String], message: &str) -> Result<&'a str, String> {
    if values.len() == 1 {
        Ok(&values[0])
    } else {
        Err(message.into())
    }
}
/// Expand option=value only where the parser owns the token, never inside data.
fn normalize_equals(args: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    let mut literal = false;
    let mut expects_value = false;
    for arg in args {
        if literal || expects_value {
            expects_value = false;
            result.push(arg);
            continue;
        }
        if arg == "--" {
            literal = true;
        }
        if let Some((flag, value)) = arg.split_once('=')
            && help::takes_value(flag)
        {
            result.push(flag.to_owned());
            result.push(value.to_owned());
            continue;
        }
        expects_value = help::takes_value(&arg);
        result.push(arg);
    }
    result
}
