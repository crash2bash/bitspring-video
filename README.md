# BitSpring Video

A video player built around one thing: showing HDR the way it was graded, on the display that is
actually connected.

It reads what the display can do (its EDID and the operating system's live HDR state), reads
everything the file says about itself (HDR10 mastering data, HDR10+ and Dolby Vision dynamic
metadata), and maps one to the other in the player with [libplacebo](https://libplacebo.org),
the renderer behind mpv and VLC. The terminal shows the whole path, from the file to the screen,
so nothing about it is a guess.

## Status

Early. What works today, on the Mac:

- 4K HEVC / H.264 / AV1 / VP9 and whatever else FFmpeg decodes, on the video decoder (VideoToolbox)
- HDR10 and HLG shown as real HDR (a BT.2020 PQ 10-bit surface with HDR10 metadata)
- HDR mapped down to SDR when the display is SDR or has HDR switched off, with a note saying so
- SDR left as SDR, never stretched
- Display detection: HDR on/off, peak and black level, gamut; rechecked every second while playing
- Sound: every codec FFmpeg decodes (AAC, AC-3, E-AC-3, DTS, TrueHD, FLAC, Opus ...), on the
  default output device, at the track's own sample rate when the device offers it; more channels
  than the device has are mixed down
- The picture follows the sound device's clock, so sound is never stretched or dropped to keep
  sync, and is drawn for the moment it will be seen rather than the moment it is drawn. Measured
  on the Mac (`scripts/sync-check.sh`): a flash within a display refresh or two of its beep
- Decoding on the platform's video decoder; where it refuses a stretch of a file that software
  reads without complaint, software takes over at the same picture and the film goes on
- Subtitles: text (SRT, ASS/SSA with its own styles and the fonts inside the file, WebVTT, MP4
  text) drawn by libass at the display's resolution; picture subtitles (Blu-ray PGS, DVD). In
  HDR, subtitle white sits at SDR white, not at the display's peak
- Subtitle files outside the video: those whose name begins with the video's (`film.srt`,
  `film.ru.ass`, `film [Group].ass`), beside it or up to three folders below it (`Subs/`,
  `RUS Subs/[Group]/`), with the fonts from a `Fonts` folder near them; and any subtitle
  file dropped on a video that is playing
- Sound files outside the video (a dubbing kept beside it: `film.mka`, `film.rus.ac3`,
  `RUS Sound/[Studio]/film.mka`), found by the same rule and listed with the video's own
  tracks; and any sound file dropped on a video that is playing. The file is read in step
  with the picture and stays with it through seeking, as closely as the video's own sound
  (`SOUND=beside scripts/sync-check.sh` measures it). One is put on unasked when the video has
  no sound, or none in the preferred language while the file is in it. The dubbing chosen
  for one episode is played with the next ones of its folder
- Choosing the sound track and the subtitle track
- Settings, each taking effect on the picture as it is changed: the display's peak brightness
  (its own figure, or yours), rendering quality, hardware decoding; subtitle size, subtitle
  brightness in HDR, preferred subtitle and sound languages; picture delay for lip sync
- Upscaling by a small neural network ([ArtCNN](https://github.com/Artoriuz/ArtCNN)), off until
  switched on in settings: a picture smaller than the window is doubled before it is fitted
  there, either as it is or with what compression left taken away. The signal panel says when
  it is at work. Tried on SDR and HDR10 at 1080p
- Frames in between, off until switched on in settings: a video at up to 30 pictures a second
  gets one made between every two (25 become 50), by the system (VideoToolbox) on the Mac,
  for 8-bit pictures up to 1920x1080. Nothing is made across a cut, and it is given up for
  a video the machine cannot make them in time for. The signal panel says which
- Pause, seeking, volume, full screen; the display and the machine stay awake while a film
  plays, and are let go on pause
- A film is picked up where it was left, with the sound and subtitle tracks chosen for it; a
  note offers its beginning instead. A film watched to its credits starts from the beginning
- Series: opening one video gathers the other videos of its folder around it, in the order a
  person reads their names (2 before 10). The next and the previous are a key away, and when
  an episode ends the next one starts by itself: where the next file's name continues this
  one's (the two differ in a number), not where two films merely share a folder
- Playlists: a folder opened whole, several files opened together, or an `.m3u8` file. A list
  can be added to, put in order and saved; it plays through, and is picked up on the file it
  was left on
- A native interface in the system's Liquid Glass (macOS 26 or later): open or drop a file,
  transport, and a signal panel that shows the video's range of light against the display's

Only files on this machine are played. A video is never allowed to send the player to the network
or to other files, whatever it claims to be. A playlist is read by the player itself, as a list
of paths on this machine: an address of anything else in it, or a path to another machine
(`\\server\films`, a folder that mounts one when asked, a link that leads to either), is passed
over without being looked for.

On Windows the same player runs through Direct3D 11, inside a native application (WinUI 3)
with the same controls, panels and settings. It builds, plays and draws its interface on the
build machine, which has neither a graphics card nor an HDR screen: HDR output, hardware
decoding and sound on Windows have not yet been judged on a real machine. It has no installer:
each time it starts it tells Windows, for the person using it only, which files it opens and
where it is now.

Not there yet: playing from the network, choosing the output device, bitstream passthrough to a
receiver.

Dolby Vision: the RPU is applied by libplacebo (profiles 5 and 8; profile 7 plays its base layer
with the RPU, the enhancement layer is not decoded). The output is HDR10 mapped to the display.
No player outside licensed devices can switch a TV into its Dolby Vision mode. Tried on
profile 8.1 (a streaming-service series): it plays with the RPU applied. Profiles 5 and 7 have
not been tried on real files, and the result has not been compared with a licensed device.
A file that is Dolby Vision and HDR10 at once (profile 8.1, and 7) can be played either way:
the signal panel offers the choice, and it is remembered.

## Layout

- `crates/display`: what the display can do (EDID parser, macOS and Windows queries) and the
  policy that picks the output signal. Pure Rust, tested.
- `crates/engine`: decoding (FFmpeg) and rendering (libplacebo over Vulkan; MoltenVK on the Mac).
  A small C bridge in `c/`, a safe Rust API on top. `shaders/artcnn` holds the upscaler's
  shaders as they are in ArtCNN at commit 0837e53, with its licence (MIT) beside them.
- `crates/audio`: sound output and the playback clock.
- `crates/player`: the window and the playback threads. A library as well as a program, so
  that the Windows application can carry the same player. `queue.rs` is what plays after what
  (folders, playlists), `sidecars.rs` finds the subtitle and sound files that belong to a video.
- `crates/player/macos`: the interface, in SwiftUI. It is compiled into the player and talks to
  it through four C functions (`src/ui.rs`): state goes in as JSON, commands come back as text.
- `crates/winhost`: the player as `bitspring_video_core.dll` for the Windows application. It
  plays into a swapchain that the application shows in one of its panels.
- `windows/BitSpringVideo`: the Windows application, in C# and WinUI 3. It talks to the core
  the way the Mac's interface does: state as JSON, commands as text.
- `scripts/`: builds the application bundle and gathers its libraries; `sync-check.sh` records
  the player's window with its sound and measures how far a flash is from its beep.
- `assets/icon/`: the app's icon and the script that draws it; and the script that draws the
  pictures for video files and playlists, one for each format with the format's name on it. A file wears one
  where this player is the one chosen to open it: the player offers itself for video files on
  both systems, and never takes a file type from the player already chosen.
- `testdata/make-clips.sh`: generates test clips: grey patches at known luminance, and a flash
  with a beep every second for checking that picture and sound coincide.

## Building (Mac)

```
brew install ffmpeg libplacebo libass molten-vk vulkan-loader pkgconf
cargo build --release
```

To build the application itself, with its icon, its libraries inside and video files offered
in Finder's "Open With":

```
scripts/bundle-macos.sh        # makes target/bundle/BitSpring Video.app
```

The libraries are taken from Homebrew as installed. Homebrew's FFmpeg includes GPL parts, so an
app built this way is for your own machine; one to give to others needs an FFmpeg built with
decoders only. It is signed without a certificate, which macOS accepts on the Mac that built it.

## Building (Windows)

Two halves that end up in one folder. The player's core, in an [MSYS2](https://www.msys2.org)
UCRT64 shell:

```
pacman -S mingw-w64-ucrt-x86_64-{rust,gcc,pkgconf,ffmpeg,libplacebo,libass,ntldd}
cargo build --release -p bitspring-video -p bsv-winhost
scripts/package-windows.sh     # the core and its libraries, in target/bundle
```

And the application, with the [.NET 8 SDK](https://dotnet.microsoft.com/download):

```
dotnet build windows/BitSpringVideo/BitSpringVideo.csproj -c Release -p:Platform=x64
```

That leaves the application in `windows/BitSpringVideo/bin/x64/Release/net8.0-windows10.0.19041.0/win-x64`.
Copy the contents of `target/bundle/BitSpring-Video-windows` into it: `BitSpringVideo.exe` there
is the player. Beside it, `bitspring-video.exe` is the same player in a bare window, for
`--displays`, `--info` and scripted runs. (Do not send the build elsewhere with `-o`: some
versions of the SDK then lose the windows' compiled markup, and the application stops at start.)

`.github/workflows/windows.yml` does all of this on every push, starts the application on a clip,
photographs it, and keeps the folder as a download.

## Running

```
bitspring-video                     open the player; drop a video on it
bitspring-video <file>              play; the other videos of its folder follow
bitspring-video <folder>            play the folder's videos, one after another
bitspring-video <a> <b> <list.m3u8> play these in this order
bitspring-video --displays          what each connected display can do
bitspring-video --info <file>       what the file is and how it would be shown
bitspring-video --hdr passthrough <file>
bitspring-video --audio-delay 80 <file>     the TV or receiver delays sound by 80 ms
```

Keys: Space pauses, Left / Right seek 5 seconds (with Shift, a minute), Up / Down change the
volume, M mutes, S steps through the subtitle tracks, A through the sound tracks, N and P go to
the next and the previous file of the folder or the playlist, L opens the list, F or a
double-click for full screen, I opens the signal panel, Cmd+comma the settings, Cmd+O a file. The controls hide after a few seconds of playback and return when the pointer moves.

Colour in the interface means one thing. Brass: the picture reaches the screen as it was
mastered. Violet: it is being changed to fit the display. The signal panel says which, and why.

Settings are kept in `~/Library/Application Support/BitSpring Video/settings.json`
(`--settings <file>` to keep them elsewhere). Options given on the command line hold for that
run and are not saved. Beside the settings, `history.json` holds where each film was stopped
and the tracks chosen for it; a film is known by its size and the beginning of its contents, so
renaming or moving it changes nothing. It also holds which file each playlist, or folder opened
whole, was left on. `--start <seconds>` begins there whatever is remembered.

For testing without a keyboard, `--script "2:pause,4:pause,5:seek+10,9:quit"` runs commands at
given times (`pin` keeps the controls up, `panel` opens the signal panel, `queue` the list;
`next`, `previous`, `play 2`, `add <file>`, `save <list>` work the list), and `--verbose` prints
the playback clock once a second.

HDR modes:

- `auto` / `tonemap`: the player maps the picture into the display's range and tells the system
  it already fits. This is the only way dynamic metadata reaches a display driven as HDR10.
- `passthrough`: the file's own metadata is sent on and the display (or the system) maps it.
- `sdr`: always SDR.

On the Mac, HDR for an external display is switched on in System Settings > Displays.
