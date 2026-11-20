//! How the player was started: what the command line asked for.

use std::path::PathBuf;

use bsv_display::policy::HdrMode;
use bsv_engine::Quality;

/// What the command line set, applied to every file opened in this run.
pub struct Settings {
    pub path: Option<String>,
    /// Where preferences are kept.
    pub preferences: PathBuf,
    /// Choices made on the command line: they hold for this run and are not saved.
    pub hardware: Option<bool>,
    pub mode: Option<HdrMode>,
    pub quality: Option<Quality>,
    pub audio_delay: Option<f64>,
    pub renderer_options: String,
    pub start: f64,
    pub sound: bool,
    /// Print the clock's state once a second.
    pub diagnostics: bool,
    pub script: Vec<Step>,
}

/// One timed command of a test script: "2:pause", "5:seek+10", "9:quit".
pub struct Step {
    pub at: f64,
    pub action: String,
}

pub fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    text.split(',')
        .map(|part| {
            let (at, action) = part
                .split_once(':')
                .ok_or(format!("\"{part}\" is not time:command"))?;
            let at = at
                .trim()
                .parse()
                .map_err(|_| format!("\"{at}\" is not a time in seconds"))?;
            Ok(Step {
                at,
                action: action.trim().to_string(),
            })
        })
        .collect()
}
