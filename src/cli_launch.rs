//! Portable project launch definitions, shared by CLI validation and the editor.
use super::*;
use std::collections::{BTreeMap, HashSet};

pub const CONFIG_PATH: &str = ".vedit/launch.json";
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_cwd() -> String {
    ".".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List,
    Configure {
        configuration: Configuration,
        #[serde(default)]
        revision: u64,
    },
    Select {
        id: String,
    },
    Start {
        #[serde(default)]
        id: Option<String>,
    },
    Stop,
    Output {
        #[serde(default = "one")]
        start_line: usize,
        #[serde(default = "one")]
        start_column: usize,
        #[serde(default = "default_limit")]
        line_count: usize,
    },
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.profiles.len() > 32
            || serde_json::to_vec_pretty(self)
                .map_err(|e| e.to_string())?
                .len() as u64
                + 1
                > MAX_CONFIG_BYTES
        {
            return Err("Launch configuration supports at most 32 profiles and 64 KiB.".into());
        }
        let mut ids = HashSet::new();
        for profile in &self.profiles {
            validate_id(&profile.id)?;
            if !ids.insert(&profile.id) {
                return Err(format!("Duplicate launch profile ID: {}", profile.id));
            }
            if profile.name.trim().is_empty()
                || profile.name.len() > 100
                || profile.name.chars().any(char::is_control)
            {
                return Err("Launch names need 1..100 bytes without control characters.".into());
            }
            if profile.program.trim().is_empty()
                || profile.program.contains(['\0', '\n', '\r'])
                || profile.program.len() > 4096
            {
                return Err("Launch program needs a nonempty executable name or path (up to 4096 bytes). Put arguments in args.".into());
            }
            if profile.cwd.is_empty()
                || profile.cwd.contains('\0')
                || profile.cwd.len() > 4096
                || profile.args.len() > 256
                || profile.args.iter().any(|s| s.contains('\0'))
            {
                return Err("Launch cwd must be nonempty; use at most 256 arguments without NUL characters.".into());
            }
            if profile
                .env
                .iter()
                .any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0'))
            {
                return Err("Launch environment names must be nonempty without '=' or NUL; values cannot contain NUL.".into());
            }
        }
        if let Some(id) = &self.default
            && !ids.contains(id)
        {
            return Err(format!("Default launch profile {id} is not in profiles."));
        }
        Ok(())
    }

    pub fn selected<'a>(&'a self, selection: Option<&str>) -> Option<&'a Profile> {
        selection
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .or_else(|| {
                self.default
                    .as_deref()
                    .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            })
            .or_else(|| self.profiles.first())
    }
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        Err("Launch ID needs 1..64 ASCII letters, digits, '-' or '_'.".into())
    } else {
        Ok(())
    }
}

pub fn decode(bytes: &[u8]) -> Result<Configuration, String> {
    let config = transport::from_unique_json::<Configuration>(bytes).map_err(|e| e.to_string())?;
    config.validate()?;
    Ok(config)
}

pub(super) fn parse(positional: &[String], options: &Options) -> Result<Action, String> {
    let operation = positional.first().map(String::as_str).unwrap_or("list");
    if operation != "configure" && (options.file.is_some() || options.revision.is_some()) {
        return Err("--file and --revision apply to launch configure.".into());
    }
    if operation != "output"
        && (options.line.is_some() || options.column.is_some() || options.count.is_some())
    {
        return Err("--line, --column and --count apply to launch output.".into());
    }
    match (operation, positional.len()) {
        ("list", 0..=1) => Ok(Action::List),
        ("configure", 1) => {
            let file = options.file.as_ref().ok_or("launch configure needs --file CONFIG.json")?;
            let configuration = transport::from_unique_json::<Configuration>(json_bytes(&read_json_file(&absolute(file.into()))?)).map_err(|e| e.to_string())?;
            Ok(Action::Configure { configuration, revision: options.revision.unwrap_or(0) })
        }
        ("select", 2) => Ok(Action::Select { id: positional[1].clone() }),
        ("start", 1..=2) => Ok(Action::Start { id: positional.get(1).cloned() }),
        ("stop", 1) => Ok(Action::Stop),
        ("output", 1) => Ok(Action::Output { start_line: options.line.unwrap_or(1), start_column: options.column.unwrap_or(1), line_count: options.count.unwrap_or(100) }),
        _ => Err("Use launch list, configure --file CONFIG.json, select ID, start [ID], stop, or output.".into()),
    }
}
