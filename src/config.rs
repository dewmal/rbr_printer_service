use crate::*;

// =========================================================
// CONFIGURATION
// =========================================================

#[derive(Clone, Copy, Debug)]
pub(crate) struct PrintSettings {
    pub(crate) paper_width_mm: f64,
    pub(crate) side_margin_mm: f64,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            paper_width_mm: DEFAULT_PAPER_WIDTH_MM,
            side_margin_mm: DEFAULT_SIDE_MARGIN_MM,
        }
    }
}

impl PrintSettings {
    pub(crate) fn content_width_mm(&self) -> f64 {
        // The configured margin is a LEFT safety margin only.
        // The remaining width on the right is fully available to the receipt.
        self.paper_width_mm - self.side_margin_mm
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !self.paper_width_mm.is_finite() || self.paper_width_mm <= 0.0 {
            bail!("Paper width must be greater than 0 mm");
        }

        if !self.side_margin_mm.is_finite() || self.side_margin_mm < 0.0 {
            bail!("Side margin cannot be negative");
        }

        if self.content_width_mm() <= 0.0 {
            bail!(
                "Left margin is too large for a {:.2} mm paper width",
                self.paper_width_mm,
            );
        }

        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) socket_server: String,
    pub(crate) socket_print_event: String,
    pub(crate) env_path: PathBuf,
    pub(crate) print_settings: PrintSettings,
}

// =========================================================
// LOAD CONFIG
// =========================================================

pub(crate) fn load_config() -> Result<Config> {
    let exe_path = env::current_exe().context("Unable to determine executable path")?;

    let exe_directory = exe_path
        .parent()
        .context("Unable to determine executable directory")?;

    // ---------------------------------------------
    // First check beside EXE
    // ---------------------------------------------

    let exe_env_path = exe_directory.join(".env");

    // ---------------------------------------------
    // Development fallback:
    // project directory
    // ---------------------------------------------

    let env_path = if exe_env_path.exists() {
        exe_env_path
    } else {
        let current_directory =
            env::current_dir().context("Unable to determine current directory")?;

        let development_path = current_directory.join(".env");

        if development_path.exists() {
            development_path
        } else {
            bail!(
                ".env not found.\nChecked:\n{}\n{}",
                exe_env_path.display(),
                development_path.display(),
            );
        }
    };

    log_info(&format!("[CONFIG] .env = {}", env_path.display(),));

    dotenvy::from_path(&env_path)
        .with_context(|| format!("Unable to load {}", env_path.display(),))?;

    // ---------------------------------------------
    // SOCKET SERVER
    // ---------------------------------------------

    let socket_server = env::var("SOCKET_SERVER")
        .context("SOCKET_SERVER missing from .env")?
        .trim()
        .trim_end_matches('/')
        .to_string();

    if socket_server.is_empty() {
        bail!("SOCKET_SERVER cannot be empty");
    }

    let server_url = Url::parse(&socket_server).context("SOCKET_SERVER is invalid")?;

    match server_url.scheme() {
        "http" | "https" | "ws" | "wss" => {}

        scheme => {
            bail!("Unsupported server scheme: {scheme}");
        }
    }

    let socket_print_event =
        env::var("SOCKET_PRINT_EVENT").unwrap_or_else(|_| "chat message".to_string());

    let paper_width_mm = read_env_f64(ENV_PAPER_WIDTH_MM, DEFAULT_PAPER_WIDTH_MM)?;

    let side_margin_mm = read_env_f64(ENV_SIDE_MARGIN_MM, DEFAULT_SIDE_MARGIN_MM)?;

    let print_settings = PrintSettings {
        paper_width_mm,
        side_margin_mm,
    };

    print_settings.validate()?;

    log_info(&format!(
        "[CONFIG] print layout = {:.2} mm paper / {:.2} mm left margin / {:.2} mm content",
        print_settings.paper_width_mm,
        print_settings.side_margin_mm,
        print_settings.content_width_mm(),
    ));

    Ok(Config {
        socket_server,

        socket_print_event,

        env_path,

        print_settings,
    })
}

// =========================================================
// PRINT SETTINGS / .ENV
// =========================================================

pub(crate) fn read_env_f64(key: &str, default_value: f64) -> Result<f64> {
    let value = match env::var(key) {
        Ok(value) => value,
        Err(_) => return Ok(default_value),
    };

    let value = value.trim();

    if value.is_empty() {
        return Ok(default_value);
    }

    value
        .parse::<f64>()
        .with_context(|| format!("{key} must be a number, got '{value}'",))
}

pub(crate) fn save_print_settings_to_env(env_path: &Path, settings: PrintSettings) -> Result<()> {
    settings.validate()?;

    let existing = fs::read_to_string(env_path)
        .with_context(|| format!("Unable to read {}", env_path.display(),))?;

    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();

    set_env_line(
        &mut lines,
        ENV_PAPER_WIDTH_MM,
        &format!("{:.2}", settings.paper_width_mm,),
    );

    set_env_line(
        &mut lines,
        ENV_SIDE_MARGIN_MM,
        &format!("{:.2}", settings.side_margin_mm,),
    );

    let mut output = lines.join("\n");

    output.push('\n');

    fs::write(env_path, output)
        .with_context(|| format!("Unable to write {}", env_path.display(),))?;

    log_info(&format!(
        "[CONFIG] Saved print settings to {}",
        env_path.display(),
    ));

    Ok(())
}

pub(crate) fn set_env_line(lines: &mut Vec<String>, key: &str, value: &str) {
    let mut found = false;

    for line in lines.iter_mut() {
        let trimmed = line.trim_start();

        if trimmed.starts_with('#') {
            continue;
        }

        let Some((existing_key, _)) = trimmed.split_once('=') else {
            continue;
        };

        if existing_key.trim() == key {
            if !found {
                *line = format!("{key}={value}",);

                found = true;
            }
        }
    }

    if !found {
        lines.push(format!("{key}={value}",));
    }
}
