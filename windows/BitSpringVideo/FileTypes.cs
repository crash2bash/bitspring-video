using System.Runtime.InteropServices;
using System.Security;
using Microsoft.Win32;

namespace BitSpringVideo;

/// <summary>
/// Tells Windows which files this player opens (videos, and playlists of them), so that it is offered under "Open with" and in
/// Settings, Default apps, and so that a video file it has been chosen for wears its picture.
///
/// The program has no installer to do this, so it does it itself each time it starts, for the
/// person using it only (nothing here needs an administrator), and only where what Windows has
/// written down differs from where the program is now: a folder that has been moved is put
/// right the next time the program runs. It never makes itself the program a file opens with:
/// Windows leaves that choice to the person, and so does this.
/// </summary>
internal static class FileTypes
{
    private const string Name = "BitSpring Video";

    /// <summary>
    /// The name drawn on the file's picture ("" for none, any other video), and the endings of
    /// the files of that format. The pictures are Assets\Document-MKV.ico and so on, drawn by
    /// assets/icon/make-document-icon.swift.
    /// </summary>
    private static readonly (string Format, string[] Endings)[] Formats =
    {
        ("MKV", new[] { ".mkv" }),
        ("WEBM", new[] { ".webm" }),
        ("MP4", new[] { ".mp4" }),
        ("M4V", new[] { ".m4v" }),
        ("MOV", new[] { ".mov" }),
        ("TS", new[] { ".ts" }),
        ("M2TS", new[] { ".m2ts", ".mts" }),
        ("AVI", new[] { ".avi" }),
        // Playlists: lists of videos, which the player also makes.
        ("M3U", new[] { ".m3u8", ".m3u" }),
        ("", new[] { ".mpg", ".mpeg", ".m2v", ".vob", ".wmv", ".flv", ".ogv", ".3gp" }),
    };

    public static void Register()
    {
        try
        {
            var program = Environment.ProcessPath;
            if (program == null) return;
            var folder = Path.GetDirectoryName(program) ?? "";
            var open = $"\"{program}\" \"%1\"";
            var changed = false;

            using var classes = Registry.CurrentUser.CreateSubKey(@"Software\Classes");
            using var capabilities = Registry.CurrentUser.CreateSubKey($@"Software\{Name}\Capabilities");

            // The program as one that can be picked for a file.
            var application = $@"Applications\{Path.GetFileName(program)}";
            changed |= Set(classes, application, "FriendlyAppName", Name);
            changed |= Set(classes, $@"{application}\shell\open\command", null, open);

            // And as one listed in Settings, Default apps.
            changed |= Set(capabilities, "", "ApplicationName", Name);
            changed |= Set(capabilities, "", "ApplicationDescription", "Plays video files, HDR as it was mastered.");
            changed |= Set(Registry.CurrentUser, @"Software\RegisteredApplications", Name, $@"Software\{Name}\Capabilities");

            foreach (var (format, endings) in Formats)
            {
                // A kind of file: what it is called, its picture, and how it is opened.
                var kind = format == "" ? "BitSpringVideo.Video" : $"BitSpringVideo.{format}";
                var picture = Path.Combine(folder, "Assets", format == "" ? "Document.ico" : $"Document-{format}.ico");
                if (!File.Exists(picture)) picture = program;
                var called = format switch { "" => "Video", "M3U" => "Playlist", _ => $"{format} video" };
                changed |= Set(classes, kind, null, called);
                changed |= Set(classes, $@"{kind}\DefaultIcon", null, $"\"{picture}\",0");
                changed |= Set(classes, $@"{kind}\shell\open\command", null, open);

                foreach (var ending in endings)
                {
                    // Offered for files with this ending, beside whatever opens them now.
                    changed |= Set(classes, $@"{ending}\OpenWithProgids", kind, "");
                    changed |= Set(classes, $@"{application}\SupportedTypes", ending, "");
                    changed |= Set(capabilities, "FileAssociations", ending, kind);
                }
            }

            // Explorer keeps what it knew about file types until told that something changed.
            if (changed) SHChangeNotify(AssociationsChanged, 0, IntPtr.Zero, IntPtr.Zero);
        }
        catch (Exception e) when (e is SecurityException or UnauthorizedAccessException or IOException)
        {
            // The registry is closed to this person: the player works without any of this.
        }
    }

    /// <summary>Write a value unless it is already there. Says whether anything was written.</summary>
    private static bool Set(RegistryKey under, string path, string? name, string value)
    {
        using var key = path == "" ? null : under.CreateSubKey(path);
        var at = key ?? under;
        if (at.GetValue(name) is string now && now == value) return false;
        at.SetValue(name, value, RegistryValueKind.String);
        return true;
    }

    private const int AssociationsChanged = 0x08000000;

    [DllImport("shell32.dll")]
    private static extern void SHChangeNotify(int what, uint how, IntPtr first, IntPtr second);
}
