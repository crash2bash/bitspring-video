// Words and figures: everything the interface says, in English and Russian.

using System.Globalization;
using System.Text.RegularExpressions;

namespace BitSpringVideo;

internal static class Words
{
    /// <summary>The system's language, unless BSV_LANG names another (for trying a translation).</summary>
    private static readonly bool Russian =
        (Environment.GetEnvironmentVariable("BSV_LANG") ?? CultureInfo.CurrentUICulture.TwoLetterISOLanguageName)
        .StartsWith("ru", StringComparison.OrdinalIgnoreCase);

    public static string L(string en, string ru) => Russian ? ru : en;

    public static readonly string NitsUnit = L("cd/m²", "кд/м²");

    private static readonly CultureInfo Plain = CultureInfo.InvariantCulture;

    /// <summary>A number with at most this many decimals and no trailing zeros.</summary>
    public static string Trimmed(double value, int places = 3) =>
        value.ToString("0." + new string('#', places), Plain);

    public static string Nits(double value) =>
        value >= 10 ? value.ToString("0", Plain) : value >= 0.1 ? Trimmed(value, 2) : Trimmed(value, 4);

    public static string Clock(double seconds)
    {
        var s = (long)Math.Max(0, Math.Floor(seconds));
        return s >= 3600 ? $"{s / 3600}:{s / 60 % 60:00}:{s % 60:00}" : $"{s / 60}:{s % 60:00}";
    }

    /// <summary>A number for the player to read back: always with a point, whatever the system's habits.</summary>
    public static string Number(double value) => value.ToString("0.###", Plain);

    public static string Kilohertz(int rate) => Trimmed(rate / 1000.0, 1) + L(" kHz", " кГц");

    /// <summary>Codec names the way they are written on a box, not the way FFmpeg spells them.</summary>
    public static string CodecName(string raw) => raw.ToLowerInvariant() switch
    {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "av1" => "AV1",
        "vp9" => "VP9",
        "vvc" => "VVC",
        "mpeg2video" => "MPEG-2",
        "eac3" => "E-AC-3",
        "ac3" => "AC-3",
        "truehd" => "TrueHD",
        "dts" => "DTS",
        "aac" => "AAC",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "mp3" => "MP3",
        var other => other.StartsWith("pcm_") ? "PCM" : raw.ToUpperInvariant(),
    };

    /// <summary>"English", "Русский": the language a track is in, from the code the file carries.</summary>
    public static string? LanguageName(string code)
    {
        if (string.IsNullOrEmpty(code) || code == "und") return null;
        // Files often use the older bibliographic codes, which the system does not know.
        var modern = code switch
        {
            "ger" => "deu", "fre" => "fra", "chi" => "zho", "cze" => "ces", "dut" => "nld",
            "gre" => "ell", "rum" => "ron", "per" => "fas", "slo" => "slk", "ice" => "isl",
            "mac" => "mkd", "may" => "msa", "arm" => "hye", "baq" => "eus", "bur" => "mya",
            "geo" => "kat", "tib" => "bod", "wel" => "cym", "alb" => "sqi",
            _ => code,
        };
        var culture = CultureInfo.GetCultures(CultureTypes.NeutralCultures).FirstOrDefault(c =>
            c.ThreeLetterISOLanguageName.Equals(modern, StringComparison.OrdinalIgnoreCase) ||
            c.TwoLetterISOLanguageName.Equals(modern, StringComparison.OrdinalIgnoreCase));
        if (culture == null) return code;
        // DisplayName comes in the system's own language where Windows has it, which is the
        // language this interface is in too; otherwise in English.
        var name = Russian ? culture.DisplayName : culture.EnglishName;
        return name.Length == 0 ? code : char.ToUpper(name[0]) + name[1..];
    }

    /// <summary>"5.1(side)" is FFmpeg's way of saying which speakers; the count is what matters here.</summary>
    public static string Speakers(string layout) => Regex.Replace(layout, @"\(.*\)", "");

    public static string TreatmentShort(Treatment t) => t.Kind switch
    {
        TreatmentKind.AsIs => L("as is", "как есть"),
        TreatmentKind.AsMastered => L("as mastered", "как задумано"),
        TreatmentKind.PassedThrough => L("passed through", "без обработки"),
        TreatmentKind.Mapped => L($"mapped to {Nits(t.To)} {NitsUnit}", $"сведено к {Nits(t.To)} {NitsUnit}"),
        _ => L("mapped down to SDR", "сведено в SDR"),
    };

    public static string SoundLabel(TrackInfo track)
    {
        var parts = new List<string>();
        if (LanguageName(track.Language) is string language) parts.Add(language);
        if (track.Title.Length > 0) parts.Add(track.Title);
        var format = CodecName(track.Codec);
        if (track.Profile.Contains("Atmos")) format += " Atmos";
        if (track.Profile.Contains("HD MA")) format += "-HD MA";
        var speakers = Speakers(track.Layout);
        parts.Add(speakers.Length == 0 ? format : $"{format} {speakers}");
        return string.Join(", ", parts);
    }

    public static string SubtitleLabel(TrackInfo track)
    {
        var parts = new List<string> { LanguageName(track.Language) ?? L($"Track {track.Id}", $"Дорожка {track.Id}") };
        if (track.Title.Length > 0) parts.Add(track.Title);
        if (track.Forced) parts.Add(L("forced", "форсированные"));
        if (track.HearingImpaired) parts.Add(L("for the hard of hearing", "для слабослышащих"));
        return string.Join(", ", parts);
    }
}
