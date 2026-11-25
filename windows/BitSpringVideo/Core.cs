// The player itself: bitspring_video_core.dll (crates/winhost), and the state it reports.
//
// The application tells it what the user did as short lines of text ("toggle-pause",
// "seek-to 12.5", "open C:\film.mkv") and reads back the player's state as JSON, a few times
// a second. The Mac's interface talks to the player in exactly the same way.

using System.Runtime.InteropServices;
using System.Text.Json;

namespace BitSpringVideo;

internal static class Native
{
    private const string Library = "bitspring_video_core";

    [DllImport(Library)]
    public static extern IntPtr bsv_host_create(
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? settings,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? log,
        int sound);

    [DllImport(Library)]
    public static extern void bsv_host_destroy(IntPtr host);

    /// <summary>
    /// Tells Windows what this thread needs kept on. 0x80000000 "until told otherwise",
    /// 0x00000002 the display, 0x00000001 the machine.
    /// </summary>
    [DllImport("kernel32.dll")]
    public static extern uint SetThreadExecutionState(uint flags);

    [DllImport(Library)]
    public static extern void bsv_host_resize(IntPtr host, int width, int height, float scale);

    [DllImport(Library)]
    public static extern void bsv_host_set_window(IntPtr host, IntPtr monitor, int fullscreen);

    [DllImport(Library)]
    public static extern void bsv_host_command(IntPtr host, [MarshalAs(UnmanagedType.LPUTF8Str)] string command);

    [DllImport(Library)]
    public static extern IntPtr bsv_host_state(IntPtr host);

    [DllImport(Library)]
    public static extern int bsv_host_panel(IntPtr host, out IntPtr swapchain);

    [DllImport("user32.dll")]
    public static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);
}

/// <summary>How a SwapChainPanel is given the swapchain to show.</summary>
[ComImport]
[Guid("63aad0b8-7c24-40ff-85a8-640d944cc325")]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface ISwapChainPanelNative
{
    [PreserveSig]
    int SetSwapChain(IntPtr swapChain);
}

/// <summary>One running player.</summary>
internal sealed class Player : IDisposable
{
    private static readonly JsonSerializerOptions Json = new()
    {
        PropertyNameCaseInsensitive = true,
        IncludeFields = true,
    };

    private IntPtr host;
    private string lastJson = "";
    private PlayerState last = new();

    public Player(string? settings, string? log, bool sound)
    {
        host = Native.bsv_host_create(settings, log, sound ? 1 : 0);
    }

    public void Send(string command)
    {
        if (host != IntPtr.Zero) Native.bsv_host_command(host, command);
    }

    public void Resize(int width, int height, float scale)
    {
        if (host != IntPtr.Zero) Native.bsv_host_resize(host, width, height, scale);
    }

    public void SetWindow(IntPtr monitor, bool fullscreen)
    {
        if (host != IntPtr.Zero) Native.bsv_host_set_window(host, monitor, fullscreen ? 1 : 0);
    }

    /// <summary>The player's state now. The same object comes back while nothing has changed.</summary>
    public PlayerState State()
    {
        if (host == IntPtr.Zero) return last;
        var json = Marshal.PtrToStringUTF8(Native.bsv_host_state(host)) ?? "";
        if (json.Length == 0 || json == lastJson) return last;
        try
        {
            last = JsonSerializer.Deserialize<PlayerState>(json, Json) ?? last;
            lastJson = json;
        }
        catch (JsonException)
        {
            // An unreadable state leaves the last good one in place.
        }
        return last;
    }

    /// <summary>What to do with the video panel: 0 nothing, 1 show this swapchain, 2 show nothing.</summary>
    public int Panel(out IntPtr swapchain)
    {
        swapchain = IntPtr.Zero;
        return host == IntPtr.Zero ? 0 : Native.bsv_host_panel(host, out swapchain);
    }

    public void Dispose()
    {
        if (host != IntPtr.Zero) Native.bsv_host_destroy(host);
        host = IntPtr.Zero;
    }
}

// The state, as crates/player/src/ui.rs writes it. Keep the two in step.
// The fields are filled in from JSON, which the compiler cannot see.
#pragma warning disable CS0649

internal sealed class PlayerState
{
    public string? File;
    public string? Error;
    public double Position;
    public double Duration;
    public bool Paused;
    public bool Ended;
    /// <summary>Where this file was picked up from, for the few seconds the note about it shows.</summary>
    public double? ResumedFrom;
    public double Volume = 1;
    public bool Muted;
    public bool Fullscreen;
    public string Mode = "auto";
    public VideoInfo? Video;
    public SignalInfo? Signal;
    public SoundInfo? Sound;
    public List<TrackInfo> AudioTracks = new();
    public List<TrackInfo> SubtitleTracks = new();
    public int? AudioTrack;
    public int? SubtitleTrack;
    /// <summary>In order; empty when the file has fewer than two.</summary>
    public List<ChapterInfo> Chapters = new();
    /// <summary>The chapter playing, by its place in Chapters.</summary>
    public int? Chapter;
    public DisplayInfo Display = new();
    public SettingsInfo Settings = new();
    /// <summary>What plays after what: the folder's videos, or a playlist.</summary>
    public QueueInfo? Queue;

    public bool IsPlaying => File != null && !Paused && !Ended;

    /// <summary>
    /// The chapter at this time, by its place in Chapters. Like the player, a time just short
    /// of a chapter's start counts as in it.
    /// </summary>
    public int? ChapterAt(double seconds)
    {
        var at = Chapters.FindLastIndex(c => c.Start <= seconds + 0.4);
        return at < 0 ? null : at;
    }
    public bool IsHdrSource => Video != null && Video.Transfer != "SDR";

    /// <summary>Brightest level the content claims, cd/m².</summary>
    public double? ContentPeak => Video?.MaxCll ?? Video?.MasteringMax;

    /// <summary>The file is Dolby Vision and is shown as such, not as the plain signal it also is.</summary>
    public bool PlaysAsDolbyVision =>
        // What reached the screen, once something has; until then, what was asked for.
        Video?.DolbyVision != null && (Signal?.DolbyVision ?? (Video.BaseLayer == null || Settings.DolbyVision));

    public string FormatName
    {
        get
        {
            if (Video == null) return "";
            if (PlaysAsDolbyVision) return "Dolby Vision";
            if (Signal?.Hdr10Plus == true) return "HDR10+";
            return Video.Transfer switch { "PQ" => "HDR10", "HLG" => "HLG", _ => "SDR" };
        }
    }

    /// <summary>
    /// The one judgement the interface makes: is the picture shown as it was mastered, or
    /// changed to fit the display.
    /// </summary>
    public Treatment? Treatment
    {
        get
        {
            if (Video == null || Signal == null) return null;
            if (Video.Transfer == "SDR") return new Treatment(TreatmentKind.AsIs);
            if (Signal.OutputTransfer == "SDR") return new Treatment(TreatmentKind.ToSdr);
            if (Signal.Passthrough) return new Treatment(TreatmentKind.PassedThrough);
            if (ContentPeak is double peak && peak > Signal.OutputPeak * 1.05)
                return new Treatment(TreatmentKind.Mapped, Signal.OutputPeak);
            return new Treatment(TreatmentKind.AsMastered);
        }
    }
}

internal enum TreatmentKind { AsIs, AsMastered, PassedThrough, Mapped, ToSdr }

internal readonly record struct Treatment(TreatmentKind Kind, double To = 0)
{
    /// <summary>Untouched pictures are brass; changed ones violet.</summary>
    public bool Untouched => Kind is TreatmentKind.AsIs or TreatmentKind.AsMastered or TreatmentKind.PassedThrough;
}

internal sealed class VideoInfo
{
    public string Codec = "";
    public int Width;
    public int Height;
    public int BitDepth;
    public double Fps;
    public string Primaries = "";
    public string Transfer = "SDR";
    public double? MasteringMin;
    public double? MasteringMax;
    public double? MaxCll;
    public string? DolbyVision;
    /// <summary>
    /// What a Dolby Vision file is without its metadata ("HDR10", "HLG", "SDR"), when it can
    /// be played that way.
    /// </summary>
    public string? BaseLayer;
    public bool EnhancementLayer;
    public bool Hardware;
}

internal sealed class SignalInfo
{
    public bool DolbyVision;
    public bool Hdr10Plus;
    public double ScenePeak;
    public string OutputTransfer = "SDR";
    public string OutputPrimaries = "";
    public int OutputBits;
    public double OutputPeak;
    public bool Passthrough;
    /// <summary>The picture on screen was doubled by the neural network before being fitted to the window.</summary>
    public bool Upscaled;
    /// <summary>Frames in between: "off", "on", "unsuited" (not for this video), "slow" (given up),
    /// "unavailable" (the method does not run on this machine).</summary>
    public string InBetween = "off";
}

internal sealed class SoundInfo
{
    public string Codec = "";
    public string Profile = "";
    public string Layout = "";
    public string Language = "";
    public int SampleRate;
    public int Channels;
    public string Device = "";
    public int DeviceRate;
    public int DeviceChannels;
}

internal sealed class TrackInfo
{
    public int Id;
    public string Codec = "";
    public string Profile = "";
    public string Language = "";
    public string Title = "";
    public string Layout = "";
    public bool Forced;
    public bool HearingImpaired;
    public bool Pictures;
}

internal sealed class ChapterInfo
{
    /// <summary>Seconds from the start.</summary>
    public double Start;
    /// <summary>Empty when the file gives it none.</summary>
    public string Title = "";
}

internal sealed class DisplayInfo
{
    public string Name = "";
    public string Hdr = "unsupported";
    public double Peak;
    public double? Min;
    public double Headroom = 1;
    public bool PeakKnown;
}

internal sealed class SettingsInfo
{
    public double? DisplayPeak;
    public double ReportedPeak;
    public string Quality = "high";
    /// <summary>"off", "faithful", "restoring": a neural network doubles pictures smaller than the window.</summary>
    public string Upscaling = "off";
    /// <summary>"off", "fast", "neural": a frame is made between every two; and which this build can do.</summary>
    public string Interpolation = "off";
    public List<string> Interpolations = new();
    public bool HardwareDecoding = true;
    /// <summary>A file that is both Dolby Vision and a plain signal is played as Dolby Vision.</summary>
    public bool DolbyVision = true;
    public double SubtitleScale = 1;
    public double SubtitleNits = 203;
    public string SubtitleLanguage = "";
    public string AudioLanguage = "";
    public double AudioDelay;
    /// <summary>When a file ends, the next one of its series or its playlist starts by itself.</summary>
    public bool AutoNext = true;
}

internal sealed class QueueItem
{
    /// <summary>What to call it in the list: its name without what it shares with its neighbours.</summary>
    public string Label = "";
    public string Name = "";
}

internal sealed class QueueInfo
{
    /// <summary>"folder" or "playlist".</summary>
    public string Kind = "folder";
    /// <summary>The folder's name or the playlist's; empty for a list not saved yet.</summary>
    public string Name = "";
    /// <summary>A playlist changed since it was read or saved.</summary>
    public bool Unsaved;
    public int? Current;
    /// <summary>Whether the next file starts by itself when this one ends.</summary>
    public bool Continues;
    public List<QueueItem> Items = new();

    public bool IsPlaylist => Kind == "playlist";
    public bool HasNext => (Current ?? Items.Count) + 1 < Items.Count;
    public bool HasPrevious => (Current ?? 0) > 0;

    /// <summary>Everything the list's panel shows, as one text: it is drawn again when this changes.</summary>
    public string Shown =>
        $"{Kind}|{Name}|{Unsaved}|{Current}|{Continues}|" + string.Join("|", Items.Select(i => i.Name));
}
