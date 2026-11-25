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

        // A file that carries two versions of the picture: Dolby Vision, and the plain signal
        // its base layer is.
        if (s.Video?.BaseLayer is string baseLayer)
        {
            panel.Children.Add(Text(L("Play as", "Показывать как"), quiet: true));
            panel.Children.Add(Choices(
                new[] { ("1", "Dolby Vision"), ("0", baseLayer) },
                s.Settings.DolbyVision ? "1" : "0",
                value => player.Send("set dolbyVision " + value)));
        }

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
                if (!s.PlaysAsDolbyVision) colour += L(" (not used)", " (не используется)");
                else if (v.EnhancementLayer) colour += L(" (base layer only)", " (только базовый слой)");
            }
            yield return (L("Colour", "Цвет"), colour);
            yield return (L("Decoding", "Декодер"),
                v.Hardware ? L("The graphics card's video decoder", "Аппаратный декодер видеокарты") : L("Software", "Программный"));
            if (s.Signal?.Upscaled == true)
            {
                var size = $"{v.Width * 2} × {v.Height * 2}";
                yield return (L("Scaling", "Масштаб"),
                    L($"Doubled to {size} by a neural network (ArtCNN)", $"Удвоено до {size} нейросетью (ArtCNN)"));
            }
            var neural = s.Settings.Interpolation == "neural";
            var motion = s.Signal?.InBetween switch
            {
                "on" => (neural
                        ? L("Frames added in between by a neural network (RIFE)", "Промежуточные кадры от нейросети (RIFE)")
                        : L("Frames added in between by the system", "Промежуточные кадры от системы"))
                    + (v.Fps > 0 ? ", " + Trimmed(v.Fps * 2) + L(" fps", " кадр/с") : ""),
                "unsuited" => L("No frames in between: only for 8-bit video at up to 30 fps",
                    "Промежуточных кадров нет: только для 8-битного видео до 30 кадр/с"),
                "slow" => L("Frames in between given up: this computer cannot make them in time",
                    "Промежуточные кадры отключены: этот компьютер не успевает их делать"),
                "unavailable" => L("No frames in between: the neural network needs a graphics card with Vulkan",
                    "Промежуточных кадров нет: нейросети нужна видеокарта с поддержкой Vulkan"),
                _ => null,
            };
            if (motion != null) yield return (L("Motion", "Движение"), motion);
            if (s.Signal?.Interlaced == true)
                yield return (L("Fields", "Поля"), L("Interlaced, made whole", "Чересстрочное, поля склеены"));
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

        var upscalingNote = Text(UpscalingNote(settings.Upscaling), 11, quiet: true);
        var picture = new List<UIElement>
        {
            peakRow, peakNote, useOwn,
            Text(L("Rendering", "Обработка"), quiet: true),
            Choices(
                new[] { ("high", L("Best", "Лучшая")), ("balanced", L("Balanced", "Средняя")), ("fast", L("Fast", "Быстрая")) },
                settings.Quality, value => Set("quality", value)),
            Text(L("Upscaling", "Апскейлинг"), quiet: true),
            Choices(
                new[] { ("off", L("Off", "Выкл")), ("faithful", L("Faithful", "Точный")), ("restoring", L("Restoring", "С очисткой")) },
                settings.Upscaling, value =>
                {
                    Set("upscaling", value);
                    upscalingNote.Text = UpscalingNote(value);
                }),
            upscalingNote,
        };
        // Only the ways of making frames in between that this build has; none, no row.
        if (settings.Interpolations.Count > 0)
        {
            var presets = new List<(string, string)> { ("off", L("Off", "Выкл")) };
            presets.AddRange(settings.Interpolations.Select(
                name => (name, name == "fast" ? L("Fast", "Быстрая") : L("Neural", "Нейросеть"))));
            picture.Add(Text(L("Motion", "Плавность"), quiet: true));
            var motionNote = Text(MotionNote(settings.Interpolation), 11, quiet: true);
            picture.Add(Choices(presets.ToArray(), settings.Interpolation, value =>
            {
                Set("interpolation", value);
                motionNote.Text = MotionNote(value);
            }));
            picture.Add(motionNote);
        }
        picture.Add(Row(L("The graphics card's video decoder", "Аппаратный декодер"), decoder,
            L("From the next file", "Со следующего файла")));
        panel.Children.Add(Group(L("Picture", "Картинка"), picture.ToArray()));

        panel.Children.Add(Group(
            L("Subtitles", "Субтитры"),
            SliderRow(L("Size", "Размер"), settings.SubtitleScale, 0.5, 2, 0.05,
                value => $"{Math.Round(value * 100)}%", value => Set("subtitleScale", Number(value))),
            SliderRow(L("Brightness in HDR", "Яркость в HDR"), settings.SubtitleNits, 60, 400, 1,
                value => $"{Nits(value)} {NitsUnit}", value => Set("subtitleNits", Number(Math.Round(value))))));

        var night = new ToggleSwitch { IsOn = settings.NightMode, OnContent = "", OffContent = "", MinWidth = 0 };
        night.Toggled += (_, _) => Set("nightMode", night.IsOn ? "1" : "0");
        panel.Children.Add(Group(
            L("Sound", "Звук"),
            Row(L("Night sound", "Ночной звук"), night),
            Text(L("Quiet speech raised and loud passages brought down, for late evenings and thin walls.",
                    "Тихая речь громче, громкие сцены тише: для позднего вечера и тонких стен."), 11, quiet: true),
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

    /// <summary>One or two sentences for the upscaling preset that is on: what it does and what it is for.</summary>
    private static string UpscalingNote(string preset) => preset switch
    {
        "faithful" => L(
            "Faithful: a neural network doubles a picture smaller than the window and adds nothing of its own. For good sources.",
            "Точный: нейросеть удваивает картинку, которая меньше окна, и ничего не добавляет от себя. Для хороших источников."),
        "restoring" => L(
            "Restoring: doubles the picture and takes away noise and what compression left. For weak sources.",
            "С очисткой: удваивает картинку и заодно убирает шум и следы сжатия. Для слабых источников."),
        _ => L(
            "A neural network can double a picture before it is fitted to the screen. It works when the window is at least 1.3 times the picture's size.",
            "Нейросеть может удваивать картинку перед подгонкой под экран. Работает, когда окно больше картинки хотя бы в 1,3 раза."),
    };

    /// <summary>The same for the way frames in between are made.</summary>
    private static string MotionNote(string preset) => preset switch
    {
        "fast" => L(
            "Fast: the system itself makes a frame between every two, so 25 a second become 50. For 8-bit video up to 1080p and 30 a second.",
            "Быстрая: система сама делает кадр между каждыми двумя, и 25 кадров в секунду становятся 50. Для 8-битного видео до 1080p и 30 кадр/с."),
        "neural" => L(
            "Neural: a neural network (RIFE) makes a frame between every two, so 25 a second become 50. 8-bit video of any size at up to 30 a second, but heavy: when the graphics card cannot keep up, frames stop being added and the signal panel says so.",
            "Нейросеть (RIFE) делает кадр между каждыми двумя, и 25 кадров в секунду становятся 50. Подходит 8-битное видео любого размера до 30 кадр/с, но это тяжело: если видеокарта не успевает, кадры перестают добавляться, и панель сигнала об этом скажет."),
        _ => L(
            "A frame can be made between every two: 25 pictures a second become 50. For videos at up to 30 a second.",
            "Между каждыми двумя кадрами можно добавлять промежуточный: 25 кадров в секунду становятся 50. Для видео до 30 кадр/с."),
    };

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

    // ---- The list of what plays next ---------------------------------------------------------

    private const double QueueRowHeight = 34;
    /// <summary>What the list's panel was last drawn from, and how far down it was scrolled.</summary>
    private string? queueShown;
    private double queueScroll;

    /// <summary>
    /// The folder's videos or the playlist: what the list is and what happens when this file
    /// ends, the files themselves (press one to play it; the button at the end of a row moves
    /// it or takes it out), and where a playlist is added to and saved.
    /// </summary>
    private UIElement QueuePanel(PlayerState s)
    {
        var panel = new StackPanel { Width = PanelWidth, Spacing = 12 };
        if (s.Queue is not QueueInfo queue) return panel;
        queueShown = queue.Shown + s.Settings.AutoNext;

        panel.Children.Add(Text(QueueTitle(queue), 15, strong: true));
        panel.Children.Add(Text(QueueExplanation(queue, s.Settings.AutoNext), 13, quiet: true));

        var rows = new StackPanel();
        for (var i = 0; i < queue.Items.Count; i++) rows.Children.Add(QueueRow(queue, i));
        var visible = Math.Clamp(Root.ActualHeight - 400, 2.5 * QueueRowHeight, 8.5 * QueueRowHeight);
        var scroller = new ScrollViewer
        {
            Content = rows,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            MaxHeight = visible,
            Margin = new Thickness(-8, 0, -8, 0),
        };
        // Drawn again after a change, the list stays where it was; opened, it shows the file playing.
        var wanted = queueScroll >= 0
            ? queueScroll
            : Math.Max(0, ((queue.Current ?? 0) + 0.5) * QueueRowHeight - visible / 2);
        scroller.Loaded += (_, _) => scroller.ChangeView(null, wanted, null, true);
        scroller.ViewChanged += (_, _) => queueScroll = scroller.VerticalOffset;
        panel.Children.Add(scroller);

        if (queue.Items.Count > 1)
        {
            panel.Children.Add(Text(L("When this one ends", "Когда это видео закончится"), quiet: true));
            panel.Children.Add(Choices(
                new[] { ("1", L("Play the next", "Играть следующее")), ("0", L("Stop", "Остановиться")) },
                s.Settings.AutoNext ? "1" : "0",
                value => player.Send("set autoNext " + value)));
        }

        var actions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var add = new Button { Content = L("Add…", "Добавить…"), FontSize = 12, Padding = new Thickness(12, 4, 12, 5) };
        add.Click += async (_, _) =>
        {
            queueFlyout.Hide();
            await ChooseFiles(adding: true);
        };
        var save = new Button
        {
            Content = queue.IsPlaylist && queue.Name.Length > 0
                ? L("Save as…", "Сохранить как…")
                : L("Save as a playlist…", "Сохранить как плейлист…"),
            FontSize = 12,
            Padding = new Thickness(12, 4, 12, 5),
        };
        save.Click += async (_, _) =>
        {
            queueFlyout.Hide();
            await SavePlaylist(queue);
        };
        actions.Children.Add(add);
        actions.Children.Add(save);
        panel.Children.Add(actions);
        return panel;
    }

    private Grid QueueRow(QueueInfo queue, int index)
    {
        var item = queue.Items[index];
        var current = queue.Current == index;
        var row = new Grid { Height = QueueRowHeight };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var words = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        words.Children.Add(new FontIcon
        {
            Glyph = "\uE768", FontSize = 10, Width = 12,
            Opacity = current ? 1 : 0,
            Foreground = Resource("PorcelainBrush"),
        });
        var label = Text(item.Label, 13, strong: current, quiet: !current);
        label.TextWrapping = TextWrapping.NoWrap;
        label.TextTrimming = TextTrimming.CharacterEllipsis;
        label.MaxWidth = PanelWidth - 84;
        words.Children.Add(label);
        var play = new Button
        {
            Content = words,
            Style = (Style)Root.Resources["OnGlass"],
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left,
            Padding = new Thickness(8, 4, 8, 5),
        };
        Tip(play, item.Name);
        play.Click += (_, _) =>
        {
            if (!current) player.Send($"queue-play {index}");
        };
        row.Children.Add(play);

        var menu = new MenuFlyout();
        void Entry(string text, bool possible, string command)
        {
            var entry = new MenuFlyoutItem { Text = text, IsEnabled = possible };
            entry.Click += (_, _) => player.Send(command);
            menu.Items.Add(entry);
        }
        Entry(L("Move up", "Выше"), index > 0, $"queue-move {index} {index - 1}");
        Entry(L("Move down", "Ниже"), index + 1 < queue.Items.Count, $"queue-move {index} {index + 1}");
        Entry(L("Take out of the list", "Убрать из списка"), !current, $"queue-remove {index}");
        var more = new Button
        {
            Content = new FontIcon { Glyph = "\uE712", FontSize = 12 },
            Style = (Style)Root.Resources["OnGlass"],
            Padding = new Thickness(8, 4, 8, 5),
            Flyout = menu,
            Opacity = 0.66,
        };
        Tip(more, L("Move or take out", "Переместить или убрать"));
        Grid.SetColumn(more, 1);
        row.Children.Add(more);
        return row;
    }
}
