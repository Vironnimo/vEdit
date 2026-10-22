//! Shell launches and independent, recoverable editor windows.
use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default, Debug)]
pub struct Arguments {
    pub paths: Vec<PathBuf>,
    pub new_window: bool,
    pub reuse_window: bool,
    pub window: Option<String>,
    pub help: bool,
    pub version: bool,
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Arguments, String> {
    let mut result = Arguments::default();
    let mut args = arguments.into_iter();
    let mut literal = false;
    while let Some(arg) = args.next() {
        if literal {
            result.paths.push(arg.into());
            continue;
        }
        match arg.to_str() {
            Some("--") => literal = true,
            Some("--new-window") | Some("-n") => result.new_window = true,
            Some("--reuse-window") | Some("-r") => result.reuse_window = true,
            Some("--help") | Some("-h") => result.help = true,
            Some("--version") => result.version = true,
            Some("--window") => {
                if result.window.is_some() {
                    return Err("Use --window once.".into());
                }
                let id = args
                    .next()
                    .and_then(|s| s.into_string().ok())
                    .ok_or("--window needs an ID from vedit-cli windows.")?;
                if !crate::profile::valid_window(&id) {
                    return Err("Invalid window ID; use vedit-cli windows.".into());
                }
                result.window = Some(id);
            }
            Some(value) if value.starts_with('-') => {
                return Err(format!(
                    "Unknown option {value}. Use -- before a filename starting with a dash."
                ));
            }
            _ => result.paths.push(arg.into()),
        }
    }
    if usize::from(result.new_window)
        + usize::from(result.reuse_window)
        + usize::from(result.window.is_some())
        > 1
    {
        return Err("Use one of --new-window, --reuse-window or --window ID.".into());
    }
    Ok(result)
}

pub const HELP: &str = "vEdit — text and code editor\nUsage: vedit [--new-window | --reuse-window | --window ID] [--] [PATH...]\n\nFiles open in their existing project or Other files. Folders open as projects.\nOmit window flags to use Settings > Layout > Open external files in a new window.\n--new-window (-n) creates an independent window with its own saved session.\n--reuse-window (-r) uses the most recently active window, or starts the main window.\n--window ID opens or restores the window shown by vedit-cli windows.\n-- ends options, including for filenames beginning with a dash.\n--help and --version print offline information.\n";

pub fn allocate_window(base: &std::path::Path) -> Result<String, String> {
    let windows = base.join("windows");
    fs::create_dir_all(&windows).map_err(|e| e.to_string())?;
    for index in 1..100_000 {
        let id = format!("w{index}");
        match fs::create_dir(windows.join(&id)) {
            Ok(()) => return Ok(id),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Too many saved windows.".into())
}

/// An unclean exit keeps this marker, so a normal main-window start can recover
/// the additional window. A deliberate close keeps its session available by ID.
pub struct OpenWindow {
    marker: PathBuf,
}
impl OpenWindow {
    pub fn mark() -> Result<Self, String> {
        let directory = crate::profile::recovery_directory();
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let marker = directory.join("window-open");
        fs::write(&marker, std::process::id().to_string()).map_err(|e| e.to_string())?;
        Ok(Self { marker })
    }
    pub fn closed(self) {
        let _ = fs::remove_file(self.marker);
    }
}

/// Returns paths only when this process owns the selected window's lock.
pub fn prepare(
    args: Arguments,
) -> Result<Option<(Vec<PathBuf>, crate::single_instance::SingleInstanceGuard)>, String> {
    let base = crate::profile::base_recovery_directory();
    let running = crate::cli::running_windows();
    let new = args.new_window
        || (!args.reuse_window
            && args.window.is_none()
            && !args.paths.is_empty()
            && crate::profile::external_open_new_window());
    let id = if new {
        allocate_window(&base)?
    } else if let Some(id) = args.window {
        if id != "main" && !crate::profile::window_directory(&base, &id).is_dir() {
            return Err(format!(
                "Window {id} does not exist. Use vedit-cli windows or vedit --new-window."
            ));
        }
        id
    } else {
        running
            .iter()
            .max_by_key(|window| window.last_active)
            .map(|window| window.id.clone())
            .unwrap_or_else(|| "main".into())
    };
    crate::profile::select_window(id.clone())?;
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(response) = crate::cli::forward_paths(args.paths.clone())? {
            if response.ok {
                return Ok(None);
            }
            return Err(response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "Could not open the requested paths.".into()));
        }
        if let Some(guard) = crate::single_instance::acquire().map_err(|error| error.to_string())? {
            return Ok(Some((args.paths, guard)));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Window {id} is still starting or closing. The requested paths were not opened; try again shortly."
            ));
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

pub fn recover_interrupted_windows() {
    if crate::profile::window_id() != "main" {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let running = crate::cli::running_windows();
    for id in crate::cli::saved_window_ids() {
        let directory =
            crate::profile::window_directory(&crate::profile::base_recovery_directory(), &id);
        if id != "main"
            && !running.iter().any(|window| window.id == id)
            && directory.join("window-open").is_file()
            && directory.join("session.json").is_file()
        {
            let _ = crate::cli::spawn_window(&executable, &id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_options_and_literal_paths_are_distinct() {
        let args =
            parse(["--new-window", "--", "--reuse-window", "C:/a b/東京.txt"].map(OsString::from))
                .unwrap();
        assert!(args.new_window);
        assert!(!args.reuse_window);
        assert_eq!(
            args.paths,
            [
                PathBuf::from("--reuse-window"),
                PathBuf::from("C:/a b/東京.txt")
            ]
        );
        for args in [
            vec!["--new-window", "--reuse-window"],
            vec!["--window", "../escape"],
            vec!["--window"],
            vec!["--window", "w0"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
    #[test]
    fn windows_keep_disjoint_profiles_and_allocate_without_replacing_sessions() {
        let base = std::env::temp_dir().join(format!("vedit-windows-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("session.json"), "primary draft").unwrap();
        let first = allocate_window(&base).unwrap();
        let one = crate::profile::window_directory(&base, &first);
        fs::write(one.join("session.json"), "secondary draft").unwrap();
        let second = allocate_window(&base).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(base.join("session.json")).unwrap(),
            "primary draft"
        );
        assert_eq!(
            fs::read_to_string(one.join("session.json")).unwrap(),
            "secondary draft"
        );
        assert!(!crate::profile::read_open_mode(&base));
        fs::write(base.join("external-open.json"), "true").unwrap();
        assert!(crate::profile::read_open_mode(&base));
        fs::remove_dir_all(base).unwrap();
    }
}
