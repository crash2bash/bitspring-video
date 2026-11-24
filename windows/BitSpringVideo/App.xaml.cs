using System.Collections;
using System.Text;
using Microsoft.UI.Xaml;

namespace BitSpringVideo;

public partial class App : Application
{
    private readonly Launch launch;
    private MainWindow? window;

    public App()
    {
        // The command line as Windows gives it to any program: a file to open, and a few
        // options for trying things out (see Launch).
        launch = Launch.Parse(Environment.GetCommandLineArgs().Skip(1).ToArray());
        InitializeComponent();
        UnhandledException += (_, e) => Note(e.Exception);
        // A resource that the markup names and cannot find is otherwise reported only as
        // "XAML parsing failed", with nothing to say which one.
        DebugSettings.IsXamlResourceReferenceTracingEnabled = true;
        DebugSettings.XamlResourceReferenceFailed += (_, e) => Note("Markup: " + e.Message);
        // Aside, so that a slow registry does not hold the window back.
        Task.Run(FileTypes.Register);
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            window = new MainWindow(launch);
            window.Activate();
        }
        catch (Exception e)
        {
            Note(e);
            throw;
        }
    }

    /// <summary>
    /// Leave word of what went wrong where it can be found afterwards: beside the log when one
    /// was asked for, otherwise in the temporary folder.
    /// </summary>
    private void Note(Exception e)
    {
        // What Windows itself said about the failure travels in the exception's data.
        var text = new StringBuilder($"{e}\n  HRESULT: 0x{e.HResult:X8}");
        foreach (DictionaryEntry entry in e.Data) text.Append($"\n  {entry.Key}: {entry.Value}");
        Note(text.ToString());
    }

    private void Note(string text)
    {
        try
        {
            var path = launch.Log != null
                ? launch.Log + ".crash.txt"
                : Path.Combine(Path.GetTempPath(), "BitSpringVideo-crash.txt");
            File.AppendAllText(path, $"{DateTime.Now:u}\n{text}\n\n");
        }
        catch (IOException)
        {
            // Nowhere to write: nothing more to do.
        }
    }
}

/// <summary>How the program was started.</summary>
public sealed record Launch(List<string> Files, string? Settings, string? Log, string? Show, bool Pin, bool Sound)
{
    public static Launch Parse(string[] args)
    {
        // What to play: a video, a folder, a playlist, or several of them.
        var files = new List<string>();
        string? settings = null, log = null, show = null;
        var pin = false;
        var sound = true;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--settings" when i + 1 < args.Length: settings = args[++i]; break;
                case "--log" when i + 1 < args.Length: log = args[++i]; break;
                // For screenshots: open a panel ("signal", "settings", "queue"), keep the controls up.
                case "--show" when i + 1 < args.Length: show = args[++i]; break;
                case "--pin": pin = true; break;
                case "--no-sound": sound = false; break;
                default:
                    if (!args[i].StartsWith("--")) files.Add(args[i]);
                    break;
            }
        }
        return new Launch(files, settings, log, show, pin, sound);
    }
}
