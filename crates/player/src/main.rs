//! BitSpring Video.
//!
//!     bitspring-video <file>             play
//!     bitspring-video --displays         what each connected display can do
//!     bitspring-video --sound-outputs    the sound outputs there are, and their ids
//!     bitspring-video --info <file>      what the file is and how it would be shown

use std::process::ExitCode;

use bsv_display::policy::{HdrMode, plan};
use bsv_engine::{Decoded, LogLevel, Quality, Read, Source, VideoDecoder};

#[cfg(any(target_os = "macos", windows))]
use bitspring_video::app;
use bitspring_video::{launch, prefs, report};

const USAGE: &str = "\
BitSpring Video

Usage:
  bitspring-video [options] [file | folder | playlist] ...
  bitspring-video --displays
  bitspring-video --sound-outputs
  bitspring-video --info <file>

Options:
  --hdr <mode>       auto (default), tonemap, passthrough, sdr
  --quality <q>      high (default), balanced, fast
  --start <seconds>  begin playing from this position
  --software         decode on the CPU instead of the video decoder
  --no-sound         play the picture only
  --settings <file>  keep settings in this file instead of the usual place
  --audio-delay <ms> hold the picture back by this much, for a TV or receiver that delays sound
  --render <opts>    extra libplacebo options, key=value,key=value
  --verbose          show renderer and decoder messages, and the clock once a second
  --script <steps>   timed commands for testing: 2:pause,4:pause,5:seek+10,6:volume-20,9:quit
                     (also: mute, hdr, fullscreen, panel, settings, queue, pin,
                     open <file>, add <file>, with | between several files, next,
                     previous, play <n> for the n-th file of the list counting from 0,
                     save <playlist>, audio <n>, subs <n>, subs off, where <n> is a
                     track number from --info (subtitle files beside the video count
                     from 1000, sound files from 2000), and set <name> <value> for a
                     setting as named in its file)

Keys:
  Space              pause
  Left / Right       back / forward 5 seconds (with Shift: 1 minute)
  F, Enter, double-click   full screen
  I                  the signal panel
  S                  next subtitle track, then off
  A                  next sound track
  N / P              the next file of the folder or the playlist / the one before
  L                  the list of what plays next
  Cmd+O              open a file
  Cmd+,              settings
  Up / Down          volume
  M                  mute
  Cmd+Q              quit
";

struct Args {
    paths: Vec<String>,
    displays: bool,
    outputs: bool,
    info: bool,
    hardware: Option<bool>,
    mode: Option<HdrMode>,
    quality: Option<Quality>,
    renderer_options: String,
    start: Option<f64>,
    sound: bool,
    audio_delay: Option<f64>,
    settings: Option<String>,
    script: String,
    log: LogLevel,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        paths: Vec::new(),
        displays: false,
        outputs: false,
        info: false,
        hardware: None,
        mode: None,
        quality: None,
        renderer_options: String::new(),
        start: None,
        sound: true,
        audio_delay: None,
        settings: None,
        script: String::new(),
        log: LogLevel::Warning,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--displays" => args.displays = true,
            "--sound-outputs" => args.outputs = true,
            "--info" => args.info = true,
            "--software" => args.hardware = Some(false),
            "--settings" => args.settings = Some(value("--settings")?),
            "--no-sound" => args.sound = false,
            "--audio-delay" => {
                let ms: f64 = value("--audio-delay")?
                    .parse()
                    .map_err(|_| "--audio-delay needs milliseconds".to_string())?;
                args.audio_delay = Some(ms / 1000.0);
            }
            "--verbose" => args.log = LogLevel::Info,
            "--debug" => args.log = LogLevel::Debug,
            "--hdr" => {
                args.mode = Some(match value("--hdr")?.as_str() {
                    "auto" => HdrMode::Auto,
                    "tonemap" => HdrMode::ToneMap,
                    "passthrough" => HdrMode::Passthrough,
                    "sdr" => HdrMode::Sdr,
                    other => return Err(format!("unknown HDR mode \"{other}\"")),
                })
            }
            "--quality" => {
                args.quality = Some(match value("--quality")?.as_str() {
                    "high" => Quality::High,
                    "balanced" => Quality::Balanced,
                    "fast" => Quality::Fast,
                    other => return Err(format!("unknown quality \"{other}\"")),
                })
            }
            "--render" => args.renderer_options = value("--render")?,
            "--script" => args.script = value("--script")?,
            "--start" => {
                args.start = Some(
                    value("--start")?
                        .parse()
                        .map_err(|_| "--start needs a number of seconds".to_string())?,
                )
            }
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path => args.paths.push(path.to_string()),
        }
    }
    Ok(args)
}

/// The id is what `soundDevice` in the settings file holds.
fn print_outputs() {
    let (all, system) = bitspring_video::sound_devices();
    if all.is_empty() {
        println!("No sound outputs.");
    }
    for device in &all {
        let mark = if system.as_ref() == Some(&device.name) {
            "  (the system's)"
        } else {
            ""
        };
        println!("{}{mark}\n  {}", device.name, device.id);
    }
}

fn print_displays() -> Result<(), String> {
    let displays = bsv_display::displays().map_err(|e| e.to_string())?;
    if displays.is_empty() {
        return Err("no display found".into());
    }
    for d in &displays {
        println!("{}", report::display(d));
    }
    Ok(())
}

fn print_info(path: &str, args: &Args) -> Result<(), String> {
    let mut source = Source::open(path).map_err(|e| e.to_string())?;
    for track in source.tracks() {
        println!("Track {}: {}", track.index, report::track(&track));
    }
    if !source.has_video() {
        return Err("the file has no video".into());
    }
    let mut decoder =
        VideoDecoder::open(&source, args.hardware.unwrap_or(true)).map_err(|e| e.to_string())?;
    // Decode up to the first frame: some HDR metadata is only inside the video bitstream.
    'first_frame: loop {
        match source.read().map_err(|e| e.to_string())? {
            Read::Video(packet) => {
                decoder.send(Some(packet)).map_err(|e| e.to_string())?;
                if let Ok(Decoded::Some(_)) = decoder.receive() {
                    break 'first_frame;
                }
            }
            Read::Audio(_) | Read::Subtitle(_) => {}
            Read::End => break 'first_frame,
        }
    }
    let info = decoder.info();
    println!("Video:   {}", report::media(&info));
    for (n, chapter) in bitspring_video::chapters::tidy(source.chapters(), info.duration)
        .iter()
        .enumerate()
    {
        println!("Chapter {}: {}", n + 1, report::chapter(chapter));
    }
    let displays = bsv_display::displays().map_err(|e| e.to_string())?;
    for d in &displays {
        let output = plan(d, &info.source(), args.mode.unwrap_or_default());
        let signal = match output.signal {
            bsv_display::policy::Signal::Hdr10 if output.passthrough => {
                "HDR10, source metadata passed through".to_string()
            }
            bsv_display::policy::Signal::Hdr10 => {
                format!("HDR10, mapped to {:.0} cd/m²", output.peak_nits)
            }
            bsv_display::policy::Signal::Sdr(p) => format!("SDR, {}", report::primaries(p)),
        };
        println!("On {}: {signal}", d.name);
        for a in &output.advice {
            println!("  Note: {}", report::advice(a));
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    bsv_engine::log_to_stderr_up_to(args.log);

    if args.displays {
        return print_displays();
    }
    if args.outputs {
        print_outputs();
        return Ok(());
    }
    if args.info {
        let Some(path) = args.paths.first().cloned() else {
            return Err("--info needs a file".into());
        };
        return print_info(&path, &args);
    }

    let script = if args.script.is_empty() {
        Vec::new()
    } else {
        launch::parse_script(&args.script)?
    };
    let settings = launch::Settings {
        paths: args.paths,
        preferences: args
            .settings
            .map(std::path::PathBuf::from)
            .unwrap_or_else(prefs::Preferences::default_path),
        hardware: args.hardware,
        mode: args.mode,
        quality: args.quality,
        audio_delay: args.audio_delay,
        renderer_options: args.renderer_options,
        start: args.start,
        sound: args.sound,
        diagnostics: args.log >= LogLevel::Info,
        script,
    };
    #[cfg(any(target_os = "macos", windows))]
    let result = app::run(settings);
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = {
        let _ = settings;
        Err("playback on this system is not built yet".into())
    };
    result
}

/// Inside an application bundle, use the Vulkan driver that ships in it rather than whatever
/// the machine happens to have installed.
#[cfg(target_os = "macos")]
fn use_bundled_vulkan_driver() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(contents) = exe.parent().and_then(|macos| macos.parent()) else {
        return;
    };
    let driver = contents.join("Resources/vulkan/icd.d/MoltenVK_icd.json");
    if driver.exists() {
        // SAFETY: the very start of `main`, before any other thread exists.
        unsafe {
            std::env::set_var("VK_DRIVER_FILES", &driver);
            std::env::set_var("VK_ICD_FILENAMES", &driver);
        }
    }
}

fn main() -> ExitCode {
    #[cfg(target_os = "macos")]
    use_bundled_vulkan_driver();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("bitspring-video: {e}");
            ExitCode::FAILURE
        }
    }
}
