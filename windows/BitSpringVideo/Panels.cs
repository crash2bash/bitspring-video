// The two panels that open from the top right: what is being done to the picture, and settings.
// Both are rebuilt from the player's state each time they open.

using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.UI;
using static BitSpringVideo.Words;

namespace BitSpringVideo;

public sealed partial class MainWindow
{
    private const double PanelWidth = 348;

    // ---- Building blocks -----------------------------------------------------------------------

    private static TextBlock Text(string text, double size = 12, bool strong = false, bool quiet = false) => new()
    {
        Text = text,
        FontSize = size,
        FontWeight = strong ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal,
        Foreground = Resource(quiet ? "QuietBrush" : "PorcelainBrush"),
        TextWrapping = TextWrapping.Wrap,
    };

    /// <summary>A label on the left, something on the right.</summary>
    private static Grid Row(string label, FrameworkElement right, string? note = null)
    {
        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var left = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        left.Children.Add(Text(label, quiet: true));
        if (note != null)
        {
            var small = Text(note, 10, quiet: true);
            small.Opacity = 0.7;
            left.Children.Add(small);
        }
        grid.Children.Add(left);
        right.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(right, 1);
        grid.Children.Add(right);
        return grid;
    }

    private static StackPanel Group(string title, params UIElement[] rows)
    {
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(Text(title, 13, strong: true));
        foreach (var row in rows) panel.Children.Add(row);
        return panel;
    }

    /// <summary>A slider under its label, with the value written out as it moves.</summary>
    private static StackPanel SliderRow(
        string label, double value, double min, double max, double step,
        Func<double, string> text, Action<double> changed)
    {
        var shown = Text(text(value), 13);
        var slider = new Slider
        {
            Minimum = min, Maximum = max, StepFrequency = step, SmallChange = step, Value = value,
            IsThumbToolTipEnabled = false,
        };
        slider.ValueChanged += (_, e) =>
        {
            shown.Text = text(e.NewValue);
            changed(e.NewValue);
        };
        var panel = new StackPanel { Spacing = 0 };
        panel.Children.Add(Row(label, shown));
        panel.Children.Add(slider);
        return panel;
    }

    /// <summary>A few choices side by side, of which one is on.</summary>
    private static Grid Choices((string Value, string Label)[] options, string selected, Action<string> chosen)
    {
        var grid = new Grid { ColumnSpacing = 4 };
        var buttons = new List<ToggleButton>();
        var on = Math.Max(0, Array.FindIndex(options, o => o.Value == selected));
        foreach (var option in options)
        {
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var button = new ToggleButton
            {
                Content = option.Label,
                IsChecked = buttons.Count == on,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                FontSize = 12,
                Padding = new Thickness(4, 5, 4, 6),
            };
            button.Click += (_, _) =>
            {
                // Pressing the one that is on leaves it on.
                foreach (var other in buttons) other.IsChecked = ReferenceEquals(other, button);
                chosen(option.Value);
            };
            Grid.SetColumn(button, buttons.Count);
            buttons.Add(button);
            grid.Children.Add(button);
        }
        return grid;
    }

    // ---- The signal panel --------------------------------------------------------------------

    private UIElement SignalPanel(PlayerState s)
    {
        var panel = new StackPanel { Width = PanelWidth, Spacing = 12 };

        var headline = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        headline.Children.Add(new Ellipse
        {
            Width = 8, Height = 8, VerticalAlignment = VerticalAlignment.Center,
            Fill = s.Treatment == null ? Resource("QuietBrush") : live,
        });
        headline.Children.Add(Text(
            s.Treatment is Treatment t ? $"{s.FormatName}, {TreatmentShort(t)}" : s.FormatName, 15, strong: true));
        panel.Children.Add(headline);
        panel.Children.Add(Text(Explanation(s), 13, quiet: true));

        if (s.IsHdrSource) panel.Children.Add(LightRuler(s));

        var rows = new Grid { ColumnSpacing = 12, RowSpacing = 6 };
        rows.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        rows.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        foreach (var (label, value) in SignalRows(s))
        {
            rows.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var row = rows.RowDefinitions.Count - 1;
            var left = Text(label, quiet: true);
            var right = Text(value);
            Grid.SetRow(left, row);
            Grid.SetRow(right, row);
            Grid.SetColumn(right, 1);
            rows.Children.Add(left);
            rows.Children.Add(right);
        }
        panel.Children.Add(rows);

        if (s.IsHdrSource)
        {
            panel.Children.Add(Text(L("Who fits HDR to the screen", "Кто подгоняет HDR под экран"), quiet: true));
            panel.Children.Add(Choices(
                new[]
                {
                    ("auto", L("The player", "Плеер")),
                    ("passthrough", L("The display", "Дисплей")),
                    ("sdr", L("Nobody: SDR", "Никто: SDR")),
                },
                s.Mode == "tonemap" ? "auto" : s.Mode,
                mode => player.Send("mode " + mode)));
        }
        return panel;
    }

    /// <summary>One or two sentences saying, in numbers, why the picture is treated the way it is.</summary>
    private static string Explanation(PlayerState s)
    {
        var d = s.Display;
        var peak = s.ContentPeak is double p ? Nits(p) : null;
        if (s.Treatment is not Treatment t) return L("Waiting for the first frame.", "Ждём первый кадр.");
        switch (t.Kind)
        {
            case TreatmentKind.AsIs:
                return L(
                    "An SDR video is shown as SDR. Nothing is stretched to look like HDR.",
                    "SDR-видео показывается как SDR. Ничего не растягивается под HDR.");
            case TreatmentKind.AsMastered:
                return peak != null
                    ? L($"The video goes up to {peak} {NitsUnit}, this display to {Nits(d.Peak)}. Every level is shown as it was graded.",
                        $"Видео доходит до {peak} {NitsUnit}, дисплей до {Nits(d.Peak)}. Каждый уровень показан так, как его выставили при цветокоррекции.")
                    : L($"The video fits within this display's {Nits(d.Peak)} {NitsUnit}.",
                        $"Видео укладывается в {Nits(d.Peak)} {NitsUnit} этого дисплея.");
            case TreatmentKind.PassedThrough:
                return L(
                    "The video and its own metadata are sent on unchanged. Mapping to the screen is left to the display.",
                    "Видео и его метаданные передаются без изменений. Сведение под экран делает сам дисплей.");
            case TreatmentKind.Mapped:
                return L(
                    $"The video goes up to {peak ?? "?"} {NitsUnit}, this display to {Nits(t.To)}. Highlights above that are compressed scene by scene; everything below stays where it was graded.",
                    $"Видео доходит до {peak ?? "?"} {NitsUnit}, дисплей до {Nits(t.To)}. Света выше этого сжимаются по сценам; всё, что ниже, остаётся на своём месте.");
            default:
                if (d.Hdr == "off")
                    return L(
                        $"HDR is switched off for {d.Name}, so the picture is mapped down to SDR. Turn it on in Settings, System, Display.",
                        $"HDR для {d.Name} выключен в системе, поэтому картинка сводится в SDR. Включите его в Параметрах: Система, Дисплей.");
                if (s.Mode == "sdr")
                    return L("You chose SDR output: the HDR picture is mapped down.", "Выбран вывод в SDR: HDR-картинка сводится вниз.");
                return L($"{d.Name} shows SDR only, so the HDR picture is mapped down.",
                         $"{d.Name} показывает только SDR, поэтому HDR-картинка сводится вниз.");
        }
    }

    private static IEnumerable<(string, string)> SignalRows(PlayerState s)
    {
        if (s.Video is VideoInfo v)
        {
            var fps = v.Fps > 0 ? ", " + Trimmed(v.Fps) + L(" fps", " кадр/с") : "";
            yield return (L("Video", "Видео"),
                $"{CodecName(v.Codec)}, {v.Width} × {v.Height}, {v.BitDepth} {L("bit", "бит")}{fps}");

            var colour = $"{v.Primaries} {v.Transfer}";
            if (v.MasteringMin is double min && v.MasteringMax is double max)
                colour += L($", mastered for {Nits(min)}–{Nits(max)} {NitsUnit}", $", мастер {Nits(min)}–{Nits(max)} {NitsUnit}");
            if (v.DolbyVision != null)
            {
                colour += L($", Dolby Vision profile {v.DolbyVision}", $", Dolby Vision, профиль {v.DolbyVision}");
                if (v.EnhancementLayer) colour += L(" (base layer only)", " (только базовый слой)");
            }
            yield return (L("Colour", "Цвет"), colour);
            yield return (L("Decoding", "Декодер"),
                v.Hardware ? L("The graphics card's video decoder", "Аппаратный декодер видеокарты") : L("Software", "Программный"));
        }
        if (s.Signal is SignalInfo o)
        {
            var text = $"{o.OutputPrimaries} {o.OutputTransfer}, {o.OutputBits} {L("bit", "бит")}";
            if (o.OutputTransfer != "SDR" && o.OutputPeak > 0)
                text += L($", up to {Nits(o.OutputPeak)} {NitsUnit}", $", до {Nits(o.OutputPeak)} {NitsUnit}");
            yield return (L("Output", "Вывод"), text);
        }
        if (s.Display.Name.Length > 0)
        {
            var text = s.Display.Name + s.Display.Hdr switch
            {
                "on" => L(", HDR on", ", HDR включён"),
                "off" => L(", HDR switched off", ", HDR выключен"),
                _ => L(", SDR only", ", только SDR"),
            };
            yield return (L("Display", "Дисплей"), text);
        }
        if (s.Sound is SoundInfo a)
        {
            var text = CodecName(a.Codec);
            if (a.Profile.Contains("Atmos")) text += " Atmos";
            if (a.Profile.Contains("HD MA")) text += "-HD MA";
            yield return (L("Sound", "Звук"), $"{text} {Speakers(a.Layout)}, {Kilohertz(a.SampleRate)}");

            var device = a.Device;
            if (a.Channels > a.DeviceChannels)
                device += a.DeviceChannels == 2
                    ? L(", mixed down to stereo", ", сведено в стерео")
                    : L($", mixed down to {a.DeviceChannels} channels", $", сведено в {a.DeviceChannels} канала");
            if (a.SampleRate != a.DeviceRate)
                device += L($", resampled to {Kilohertz(a.DeviceRate)}", $", пересчитано в {Kilohertz(a.DeviceRate)}");
            yield return (L("Plays on", "Играет на"), device);
        }
    }

    /// <summary>
    /// The video's range of light against the display's, on one logarithmic scale. The bar for
    /// the video is brass where the display shows it as graded and violet where it has to be
    /// compressed. A tick marks the current scene's peak when the file says what it is.
    /// </summary>
    private static Canvas LightRuler(PlayerState s)
    {
        const double low = 0.001, high = 10000, labels = 62, width = PanelWidth;
        double X(double value) =>
            labels + (Math.Log10(Math.Clamp(value, low, high)) - Math.Log10(low)) /
            (Math.Log10(high) - Math.Log10(low)) * (width - labels);

        var canvas = new Canvas { Width = width, Height = 62 };
        void Place(FrameworkElement element, double x, double y)
        {
            Canvas.SetLeft(element, x);
            Canvas.SetTop(element, y);
            canvas.Children.Add(element);
        }
        void Bar(double from, double to, double y, Brush fill)
        {
            if (to <= from) return;
            Place(new Rectangle { Width = Math.Max(X(to) - X(from), 2), Height = 8, RadiusX = 4, RadiusY = 4, Fill = fill }, X(from), y);
        }

        // Decade lines, with a figure under a few of them.
        var line = new SolidColorBrush(Color.FromArgb(36, 0xE8, 0xED, 0xEA));
        for (var exponent = -3; exponent <= 4; exponent++)
            Place(new Rectangle { Width = 1, Height = 40, Fill = line }, X(Math.Pow(10, exponent)), 0);
        foreach (var (value, text) in new[] { (0.01, "0.01"), (1.0, "1"), (100.0, "100"), (10000.0, "10 000") })
        {
            var figure = Text(text, 11, quiet: true);
            figure.TextWrapping = TextWrapping.NoWrap;
            figure.Width = 60;
            figure.TextAlignment = value >= high ? TextAlignment.Right : TextAlignment.Center;
            Place(figure, value >= high ? X(value) - 60 : X(value) - 30, 44);
        }
        Place(Text(NitsUnit, 11, quiet: true), 0, 44);

        // The video.
        var contentMin = s.Video?.MasteringMin ?? 0.005;
        var contentMax = s.ContentPeak ?? 1000;
        var limit = s.Treatment is Treatment t
            ? t.Kind == TreatmentKind.Mapped ? t.To : t.Kind == TreatmentKind.ToSdr ? 203 : double.PositiveInfinity
            : double.PositiveInfinity;
        Place(Text(L("Video", "Видео"), 11), 0, 2);
        Bar(contentMin, Math.Min(contentMax, limit), 6, Resource("BrassBrush"));
        if (contentMax > limit) Bar(limit, contentMax, 6, Resource("VioletBrush"));
        if (s.Signal is { ScenePeak: > 0 } signal)
            Place(new Rectangle { Width = 2, Height = 16, Fill = Resource("PorcelainBrush") }, X(signal.ScenePeak), 2);

        // The display.
        var displayMax = s.Display.Hdr == "on" ? s.Display.Peak : 203;
        Place(Text(L("Display", "Дисплей"), 11), 0, 22);
        Bar(s.Display.Min ?? 0.05, displayMax, 26, new SolidColorBrush(Color.FromArgb(107, 0xE8, 0xED, 0xEA)));
        return canvas;
    }

    // ---- Settings ----------------------------------------------------------------------------

    private UIElement SettingsPanel(PlayerState s)
    {
        var settings = s.Settings;
        void Set(string key, string value) => player.Send($"set {key} {value}");

        var panel = new StackPanel { Width = PanelWidth, Spacing = 18 };
        panel.Children.Add(Text(L("Settings", "Настройки"), 15, strong: true));

        // The display's peak: brightness is judged in ratios, so the slider moves in them too.
        const double low = 200, high = 4000;
        var peak = settings.DisplayPeak ?? Math.Max(settings.ReportedPeak, low);
        var peakRow = SliderRow(
            L("Display peak", "Пик дисплея"),
            Math.Log(Math.Clamp(peak, low, high) / low) / Math.Log(high / low), 0, 1, 0.001,
            position => $"{Nits(Math.Round(low * Math.Pow(high / low, position) / 10) * 10)} {NitsUnit}",
            position => Set("displayPeak", Number(Math.Round(low * Math.Pow(high / low, position) / 10) * 10)));
        var peakNote = Text(
            L("HDR is fitted to this level. Set it yourself if highlights clip or the picture looks dim.",
              "Под этот уровень подгоняется HDR. Задайте его сами, если света срезаются или картинка тусклая."),
            11, quiet: true);
        var useOwn = new Button
        {
            Content = L($"Use the display's {Nits(settings.ReportedPeak)}", $"Вернуть {Nits(settings.ReportedPeak)}"),
            FontSize = 11,
            Padding = new Thickness(10, 4, 10, 4),
            HorizontalAlignment = HorizontalAlignment.Left,
            Visibility = settings.DisplayPeak != null ? Visibility.Visible : Visibility.Collapsed,
        };
        useOwn.Click += (_, _) =>
        {
            Set("displayPeak", "auto");
            settingsFlyout.Hide();
        };

        var decoder = new ToggleSwitch { IsOn = settings.HardwareDecoding, OnContent = "", OffContent = "", MinWidth = 0 };
        decoder.Toggled += (_, _) => Set("hardwareDecoding", decoder.IsOn ? "1" : "0");

        panel.Children.Add(Group(
            L("Picture", "Картинка"),
            peakRow, peakNote, useOwn,
            Text(L("Rendering", "Обработка"), quiet: true),
            Choices(
                new[] { ("high", L("Best", "Лучшая")), ("balanced", L("Balanced", "Средняя")), ("fast", L("Fast", "Быстрая")) },
                settings.Quality, value => Set("quality", value)),
            Row(L("The graphics card's video decoder", "Аппаратный декодер"), decoder,
                L("From the next file", "Со следующего файла"))));

        panel.Children.Add(Group(
            L("Subtitles", "Субтитры"),
            SliderRow(L("Size", "Размер"), settings.SubtitleScale, 0.5, 2, 0.05,
                value => $"{Math.Round(value * 100)}%", value => Set("subtitleScale", Number(value))),
            SliderRow(L("Brightness in HDR", "Яркость в HDR"), settings.SubtitleNits, 60, 400, 1,
                value => $"{Nits(value)} {NitsUnit}", value => Set("subtitleNits", Number(Math.Round(value))))));

        panel.Children.Add(Group(
            L("Sound", "Звук"),
            SliderRow(L("Picture delay", "Задержка картинки"), settings.AudioDelay, -200, 500, 10,
                value => $"{Math.Round(value)} {L("ms", "мс")}", value => Set("audioDelay", Number(Math.Round(value)))),
            Text(L("For a TV or receiver that delays sound.", "Для телевизора или ресивера, который задерживает звук."),
                11, quiet: true)));

        // Rules for files yet to be opened, kept apart from what acts on the one playing:
        // beside the size and the delay they read as a choice for the film on screen.
        panel.Children.Add(Group(
            L("When a file opens", "При открытии файла"),
            Row(L("Sound track", "Звуковая дорожка"),
                LanguageBox(L("As the file prefers", "Как в файле"), settings.AudioLanguage,
                    value => Set("audioLanguage", value))),
            Row(L("Subtitles", "Субтитры"),
                LanguageBox(L("Only when forced", "Только форсированные"), settings.SubtitleLanguage,
                    value => Set("subtitleLanguage", value))),
            Text(L("Which tracks a file starts with. While it plays, change them from the bar below.",
                   "С какими дорожками файл начинается. Во время просмотра они меняются на панели внизу."),
                11, quiet: true)));

        // As tall as the window lets it be, so that the last group is not left under a scroll
        // nothing points at; in a short window it scrolls. The margin keeps the flyout, which
        // moves up when it has no room below the gear, clear of the window's own buttons.
        return new ScrollViewer
        {
            Content = panel,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            MaxHeight = Math.Max(240, Root.ActualHeight - 128),
        };
    }

    /// <summary>A list of languages, with one more choice at the top for "no preference".</summary>
    private static ComboBox LanguageBox(string none, string selected, Action<string> chosen)
    {
        // The languages films usually come in. The player matches any spelling of a code.
        var codes = new[] { "eng", "rus", "ukr", "kat", "deu", "fra", "spa", "ita", "por", "pol", "tur", "jpn", "kor", "zho" };
        var languages = codes.Select(code => (Code: code, Name: LanguageName(code) ?? code)).OrderBy(l => l.Name).ToList();
        languages.Insert(0, ("", none));

        var box = new ComboBox { MinWidth = 150 };
        foreach (var language in languages) box.Items.Add(language.Name);
        box.SelectedIndex = Math.Max(0, languages.FindIndex(l => l.Code == selected));
        box.SelectionChanged += (_, _) =>
        {
            if (box.SelectedIndex >= 0) chosen(languages[box.SelectedIndex].Code);
        };
        return box;
    }
}
