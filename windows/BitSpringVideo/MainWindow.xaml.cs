using System.Runtime.InteropServices;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;
using Windows.Storage.Pickers;
using Windows.System;
using Windows.UI;
using WinRT;
using static BitSpringVideo.Words;

namespace BitSpringVideo;

public sealed partial class MainWindow : Window
{
    private static readonly Color Porcelain = Color.FromArgb(255, 0xE8, 0xED, 0xEA);
    private static readonly Color Brass = Color.FromArgb(255, 0xC9, 0xA4, 0x5C);
    private static readonly Color Violet = Color.FromArgb(255, 0x8C, 0x7A, 0xE6);
    private static readonly Color Slate = Color.FromArgb(255, 0x12, 0x1D, 0x23);

    private readonly Player player;
    private readonly Launch launch;
    private readonly DispatcherTimer tick = new() { Interval = TimeSpan.FromMilliseconds(100) };
    private bool closed;

    /// <summary>The colour of what is playing: brass, violet, or porcelain until it is known.</summary>
    private readonly SolidColorBrush live = new(Porcelain);

    private PlayerState state = new();
    private PlayerState? applied;
    /// <summary>The sliders are being set from the player's state, not moved by the user.</summary>
    private bool updating;
    private bool seeking;
    private double? seekTarget;
    private DateTime seekAsked = DateTime.MinValue;
    private double? seekWaiting;
    private DateTime seekSent = DateTime.MinValue;
    private bool showsRemaining;

    private bool controlsVisible = true;
    private DateTime lastPointer = DateTime.UtcNow;
    private int hovering;
    private int flyoutsOpen;
    private string? showWhenReady;
    private readonly DateTime started = DateTime.UtcNow;

    private readonly Flyout signalFlyout = new() { Placement = FlyoutPlacementMode.BottomEdgeAlignedRight };
    private readonly Flyout settingsFlyout = new() { Placement = FlyoutPlacementMode.BottomEdgeAlignedRight };
    private readonly Flyout queueFlyout = new() { Placement = FlyoutPlacementMode.BottomEdgeAlignedLeft };
    private bool queueOpen;

    public MainWindow(Launch launch)
    {
        this.launch = launch;
        InitializeComponent();

        Title = "BitSpring Video";
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(DragRegion);
        // The window's own buttons sit over a dark picture whatever theme the system is in:
        // left alone they would be drawn dark on dark when the system's theme is light.
        var bar = AppWindow.TitleBar;
        bar.ButtonBackgroundColor = Microsoft.UI.Colors.Transparent;
        bar.ButtonInactiveBackgroundColor = Microsoft.UI.Colors.Transparent;
        bar.ButtonForegroundColor = Porcelain;
        bar.ButtonInactiveForegroundColor = Color.FromArgb(255, 0x9A, 0xA3, 0x9F);
        bar.ButtonHoverBackgroundColor = Color.FromArgb(255, 0x2C, 0x38, 0x3E);
        bar.ButtonHoverForegroundColor = Porcelain;
        bar.ButtonPressedBackgroundColor = Color.FromArgb(255, 0x3D, 0x4A, 0x51);
        bar.ButtonPressedForegroundColor = Porcelain;
        var icon = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "icon.ico");
        if (File.Exists(icon)) AppWindow.SetIcon(icon);
        FitToScreen();

        player = new Player(launch.Settings, launch.Log, launch.Sound);

        EmptyTitle.Text = L("Drop a video here", "Перетащите сюда видео");
        EmptyHint.Text = L(
            "Or a folder of them, or a playlist. Subtitles can be dropped on a video that is playing.",
            "Или папку с ними, или плейлист. Субтитры можно бросить на видео, которое уже играет.");
        OpenButton.Content = L("Open…", "Открыть…");
        RestartButton.Content = L("From the start", "С начала");
        Tip(TitleButton, L("Open another video, several, or a playlist", "Открыть другое видео, несколько или плейлист"));
        Tip(QueueButton, L("What plays next", "Что играет дальше"));
        Tip(PreviousButton, L("The one before (P)", "Предыдущее (P)"));
        Tip(NextButton, L("The next one (N)", "Следующее (N)"));
        Tip(SignalButton, L("How the picture is being shown", "Как показывается картинка"));
        Tip(SettingsButton, L("Settings", "Настройки"));
        Tip(DurationButton, L("Switch between length and time left", "Длительность или оставшееся время"));
        Tip(SubtitlesButton, L("Subtitles", "Субтитры"));
        Tip(SoundButton, L("Sound track", "Звуковая дорожка"));

        // The play key and the played part of the seek bar take the signal's colour.
        PlayButton.Foreground = new SolidColorBrush(Slate);
        foreach (var key in new[] { "ButtonBackground", "ButtonBackgroundPointerOver", "ButtonBackgroundPressed" })
            PlayButton.Resources[key] = live;
        foreach (var key in new[] { "ButtonForeground", "ButtonForegroundPointerOver", "ButtonForegroundPressed" })
            PlayButton.Resources[key] = new SolidColorBrush(Slate);
        foreach (var key in new[] { "SliderTrackValueFill", "SliderTrackValueFillPointerOver", "SliderTrackValueFillPressed" })
            Seek.Resources[key] = live;
        Seek.ThumbToolTipValueConverter = new ClockConverter(() => state.Duration);
        // A slider keeps pointer events to itself; ask for them anyway to know when a drag ends.
        Seek.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, _) => seeking = true), true);
        Seek.AddHandler(UIElement.PointerReleasedEvent, new PointerEventHandler((_, _) => EndSeek()), true);
        Seek.AddHandler(UIElement.PointerCaptureLostEvent, new PointerEventHandler((_, _) => EndSeek()), true);

        SignalButton.Flyout = signalFlyout;
        SettingsButton.Flyout = settingsFlyout;
        QueueButton.Flyout = queueFlyout;
        queueFlyout.Opening += (_, _) =>
        {
            queueScroll = -1;
            queueFlyout.Content = QueuePanel(state);
        };
        queueFlyout.Opened += (_, _) => queueOpen = true;
        queueFlyout.Closed += (_, _) => queueOpen = false;
        signalFlyout.Opening += (_, _) => signalFlyout.Content = SignalPanel(state);
        settingsFlyout.Opening += (_, _) => settingsFlyout.Content = SettingsPanel(state);
        var subtitlesMenu = new MenuFlyout();
        subtitlesMenu.Opening += (_, _) => FillSubtitlesMenu(subtitlesMenu);
        SubtitlesButton.Flyout = subtitlesMenu;
        var soundMenu = new MenuFlyout();
        soundMenu.Opening += (_, _) => FillSoundMenu(soundMenu);
        SoundButton.Flyout = soundMenu;
        foreach (var flyout in new FlyoutBase[] { signalFlyout, settingsFlyout, queueFlyout, subtitlesMenu, soundMenu })
        {
            flyout.Opened += (_, _) => flyoutsOpen++;
            flyout.Closed += (_, _) => flyoutsOpen = Math.Max(0, flyoutsOpen - 1);
        }

        // While the pointer rests on a control, the controls stay.
        foreach (var piece in new FrameworkElement[] { TitlePill, QueuePill, SignalPill, SettingsPill, Transport })
        {
            piece.PointerEntered += (_, _) => hovering++;
            piece.PointerExited += (_, _) => hovering = Math.Max(0, hovering - 1);
        }
        Chrome.OpacityTransition = new ScalarTransition { Duration = TimeSpan.FromMilliseconds(300) };
        SettingsPill.OpacityTransition = new ScalarTransition { Duration = TimeSpan.FromMilliseconds(300) };

        Video.SizeChanged += (_, _) => ResizeVideo();
        Video.CompositionScaleChanged += (_, _) => ResizeVideo();
        AddKeys();

        tick.Tick += (_, _) => OnTick();
        tick.Start();
        Closed += (_, _) =>
        {
            closed = true;
            tick.Stop();
            player.Dispose();
        };

        Apply(state);
        if (launch.Files.Count > 0) player.Send("open " + string.Join("\n", launch.Files));
        showWhenReady = launch.Show;
    }

    /// <summary>For screenshots: open the panel named on the command line once there is something to show.</summary>
    private void ShowWhenReady()
    {
        if ((DateTime.UtcNow - started).TotalSeconds < 2) return;
        switch (showWhenReady)
        {
            case "queue" when state.Queue != null && state.Signal != null:
                queueFlyout.ShowAt(QueueButton);
                showWhenReady = null;
                break;
            case "settings" when launch.Files.Count == 0 || state.Signal != null:
                settingsFlyout.ShowAt(SettingsButton);
                showWhenReady = null;
                break;
            case "signal" when state.Signal != null:
                signalFlyout.ShowAt(SignalButton);
                showWhenReady = null;
                break;
        }
    }

    // ---- The window ----------------------------------------------------------------------

    private bool IsFullscreen => AppWindow.Presenter.Kind == AppWindowPresenterKind.FullScreen;

    private void FitToScreen()
    {
        var area = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        var width = Math.Min(1280, (int)(area.Width * 0.9));
        var height = Math.Min(760, (int)(area.Height * 0.9));
        AppWindow.Resize(new SizeInt32(width, height));
    }

    private void ToggleFullscreen() =>
        AppWindow.SetPresenter(IsFullscreen ? AppWindowPresenterKind.Default : AppWindowPresenterKind.FullScreen);

    private void ResizeVideo()
    {
        var scale = Video.CompositionScaleX;
        player.Resize((int)(Video.ActualWidth * scale), (int)(Video.ActualHeight * Video.CompositionScaleY), scale);
    }

    private static void Tip(DependencyObject element, string text) => ToolTipService.SetToolTip(element, text);

    private static Brush Resource(string key) => (Brush)Application.Current.Resources[key];

    // ---- Keeping in step with the player ---------------------------------------------------

    private void OnTick()
    {
        // A tick already on its way when the window closed still arrives, with no player to ask.
        if (closed) return;
        var window = WinRT.Interop.WindowNative.GetWindowHandle(this);
        player.SetWindow(Native.MonitorFromWindow(window, 2 /* nearest */), IsFullscreen);

        switch (player.Panel(out var swapchain))
        {
            case 1:
                Video.As<ISwapChainPanelNative>().SetSwapChain(swapchain);
                Marshal.Release(swapchain);
                ResizeVideo();
                break;
            case 2:
                Video.As<ISwapChainPanelNative>().SetSwapChain(IntPtr.Zero);
                break;
        }

        // A seek asked for while the last one was still on its way.
        if (seekWaiting is double waiting && (DateTime.UtcNow - seekSent).TotalMilliseconds > 150)
        {
            seekWaiting = null;
            SendSeek(waiting);
        }

        state = player.State();
        if (!ReferenceEquals(state, applied))
        {
            Apply(state);
            applied = state;
        }
        KeepOrHideControls();
        if (showWhenReady != null) ShowWhenReady();
    }

    /// <summary>
    /// While a film plays nobody touches the keyboard, and without this the display goes dark
    /// and the machine to sleep in the middle of it. Let go on pause and at the end.
    /// </summary>
    private void KeepAwake(bool wanted)
    {
        if (wanted == awake) return;
        awake = wanted;
        const uint untilToldOtherwise = 0x80000000, display = 0x00000002, machine = 0x00000001;
        Native.SetThreadExecutionState(wanted ? untilToldOtherwise | display | machine : untilToldOtherwise);
    }

    private bool awake;

    private void Apply(PlayerState s)
    {
        KeepAwake(s.IsPlaying);
        var open = s.File != null;
        Empty.Visibility = open ? Visibility.Collapsed : Visibility.Visible;
        Chrome.Visibility = open ? Visibility.Visible : Visibility.Collapsed;
        AppWindow.Title = open ? $"{s.File} - BitSpring Video" : "BitSpring Video";

        EmptyDisplay.Text = DisplayLine(s.Display);
        EmptyDisplay.Foreground = s.Display.Hdr == "off" ? Resource("BrassBrush") : Resource("QuietBrush");
        EmptyError.Text = s.Error ?? "";
        EmptyError.Visibility = s.Error == null ? Visibility.Collapsed : Visibility.Visible;
        ResumeNote.Visibility = open && s.ResumedFrom != null ? Visibility.Visible : Visibility.Collapsed;
        if (s.ResumedFrom is double from)
            ResumeText.Text = L($"Picked up from {Clock(from)}", $"Продолжено с {Clock(from)}");
        if (!open) return;

        TitleText.Text = s.File;
        // Where the file stands among the others, when there are others.
        var queue = s.Queue;
        var several = queue != null && queue.Items.Count > 1;
        QueueText.Text = several && queue!.Current is int at ? $"{at + 1} / {queue.Items.Count}" : "";
        QueueText.Visibility = QueueText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        QueueGlyph.Visibility = QueueText.Text.Length > 0 ? Visibility.Collapsed : Visibility.Visible;
        PreviousButton.Visibility = several ? Visibility.Visible : Visibility.Collapsed;
        NextButton.Visibility = several ? Visibility.Visible : Visibility.Collapsed;
        PreviousButton.IsEnabled = queue?.HasPrevious == true;
        NextButton.IsEnabled = queue?.HasNext == true;
        // The open list follows what was done to it.
        if (queueOpen && queue != null && queue.Shown + s.Settings.AutoNext != queueShown)
            queueFlyout.Content = QueuePanel(s);
        var treatment = s.Treatment;
        live.Color = treatment is Treatment t ? (t.Untouched ? Brass : Violet) : Porcelain;

        SignalPill.Visibility = s.Video != null ? Visibility.Visible : Visibility.Collapsed;
        SignalDot.Fill = treatment == null ? Resource("QuietBrush") : live;
        SignalFormat.Text = s.FormatName;
        SignalTreatment.Text = treatment is Treatment known ? TreatmentShort(known) : "";

        PlayGlyph.Glyph = s.Ended ? "" : s.Paused ? "" : "";
        Tip(PlayButton, s.Paused ? L("Play", "Играть") : L("Pause", "Пауза"));

        // The seek bar stands where the user put it until playback gets there.
        if (seekTarget is double target &&
            (Math.Abs(s.Position - target) < 1 || (DateTime.UtcNow - seekAsked).TotalSeconds > 2.5))
            seekTarget = null;
        var position = seekTarget ?? s.Position;
        Elapsed.Text = Clock(position);
        DurationText.Text = showsRemaining ? "−" + Clock(s.Duration - position) : Clock(s.Duration);

        updating = true;
        if (!seeking) Seek.Value = s.Duration > 0 ? position / s.Duration : 0;
        Volume.Value = s.Muted ? 0 : s.Volume;
        updating = false;

        var sound = s.Sound != null ? Visibility.Visible : Visibility.Collapsed;
        MuteButton.Visibility = sound;
        Volume.Visibility = sound;
        MuteGlyph.Glyph = s.Muted || s.Volume <= 0.001 ? "" : "";
        Tip(MuteButton, s.Muted ? L("Unmute", "Включить звук") : L("Mute", "Без звука"));

        SubtitlesButton.Visibility = s.SubtitleTracks.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        SubtitlesGlyph.Foreground = s.SubtitleTrack == null ? Resource("QuietBrush") : Resource("PorcelainBrush");
        // More than one track to choose from, or one that is not playing (a sound file beside
        // the video that could not be put on by itself).
        SoundButton.Visibility = s.AudioTracks.Count > 1 || s.AudioTracks.Any(t => t.Id != s.AudioTrack)
            ? Visibility.Visible : Visibility.Collapsed;

        FullscreenGlyph.Glyph = IsFullscreen ? "" : "";
        Tip(FullscreenButton, IsFullscreen ? L("Leave full screen", "Выйти из полного экрана") : L("Full screen", "На весь экран"));
    }

    /// <summary>The first thing the player says is what the connected display can do.</summary>
    private static string DisplayLine(DisplayInfo d)
    {
        if (d.Name.Length == 0) return "";
        return d.Hdr switch
        {
            "on" when d.PeakKnown => L(
                $"{d.Name} is showing HDR, up to {Nits(d.Peak)} {NitsUnit}.",
                $"{d.Name} показывает HDR, до {Nits(d.Peak)} {NitsUnit}."),
            "on" => L(
                $"{d.Name} is showing HDR. It does not say how bright it gets.",
                $"{d.Name} показывает HDR. Свою яркость он не сообщает."),
            "off" => L(
                $"{d.Name} can show HDR, but it is switched off. Turn it on in Settings, System, Display.",
                $"{d.Name} умеет HDR, но он выключен. Включите его в Параметрах: Система, Дисплей."),
            _ => L(
                $"{d.Name} shows SDR. HDR video will be mapped down to it.",
                $"{d.Name} показывает SDR. HDR-видео будет сведено под него."),
        };
    }

    // ---- Showing and hiding the controls -------------------------------------------------------

    private void KeepOrHideControls()
    {
        var keep = !state.IsPlaying || hovering > 0 || flyoutsOpen > 0 || launch.Pin ||
                   (DateTime.UtcNow - lastPointer).TotalSeconds < 2.6;
        if (keep == controlsVisible) return;
        controlsVisible = keep;
        Chrome.Opacity = keep ? 1 : 0;
        Chrome.IsHitTestVisible = keep;
        SettingsPill.Opacity = keep ? 1 : 0;
        SettingsPill.IsHitTestVisible = keep;
        // Subtitles move up out of the controls' way, and back down when they go.
        player.Send(keep ? "controls 1" : "controls 0");
    }

    private void OnPointerMoved(object sender, PointerRoutedEventArgs e) => lastPointer = DateTime.UtcNow;

    // ---- What the user does ------------------------------------------------------------------

    private void OnPlayClick(object sender, RoutedEventArgs e) => PlayOrPause();

    private void PlayOrPause() => player.Send(state.Ended ? "seek-to 0" : "toggle-pause");

    private void OnMuteClick(object sender, RoutedEventArgs e) => player.Send("toggle-mute");

    private void OnPreviousClick(object sender, RoutedEventArgs e) => player.Send("previous");

    private void OnNextClick(object sender, RoutedEventArgs e) => player.Send("next");

    private void OnFullscreenClick(object sender, RoutedEventArgs e) => ToggleFullscreen();

    private void OnRestartClick(object sender, RoutedEventArgs e) => player.Send("seek-to 0");

    private void OnDurationClick(object sender, RoutedEventArgs e)
    {
        showsRemaining = !showsRemaining;
        Apply(state);
    }

    private void OnDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        // Only on the picture itself, not on a control that happens to be clicked twice.
        if (ReferenceEquals(e.OriginalSource, Video) || ReferenceEquals(e.OriginalSource, Root) ||
            ReferenceEquals(e.OriginalSource, DragRegion))
            ToggleFullscreen();
    }

    private void OnSeekChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (updating || state.Duration <= 0) return;
        var seconds = e.NewValue * state.Duration;
        seekTarget = seconds;
        seekAsked = DateTime.UtcNow;
        Elapsed.Text = Clock(seconds);
        // While dragging, follow along a few times a second.
        if ((DateTime.UtcNow - seekSent).TotalMilliseconds > 150) SendSeek(seconds);
        else seekWaiting = seconds;
    }

    private void SendSeek(double seconds)
    {
        seekSent = DateTime.UtcNow;
        player.Send("seek-to " + Number(seconds));
    }

    private void EndSeek()
    {
        seeking = false;
        if (seekWaiting is double waiting)
        {
            seekWaiting = null;
            SendSeek(waiting);
        }
    }

    private void OnVolumeChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!updating) player.Send("volume " + Number(e.NewValue));
    }

    private async void OnOpenClick(object sender, RoutedEventArgs e) => await ChooseFiles(adding: false);

    /// <summary>
    /// Choose what to play, or what to put at the end of the list: videos, a playlist, or a
    /// subtitle file for the video that is playing. (A folder is opened by dropping it.)
    /// </summary>
    private async Task ChooseFiles(bool adding)
    {
        var picker = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.VideosLibrary };
        picker.FileTypeFilter.Add("*");
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        var files = await picker.PickMultipleFilesAsync();
        if (files.Count > 0)
            player.Send((adding ? "queue-add " : "open ") + string.Join("\n", files.Select(f => f.Path)));
    }

    /// <summary>Write the list to a playlist file: plain text, one path to a line.</summary>
    private async Task SavePlaylist(QueueInfo queue)
    {
        var picker = new FileSavePicker
        {
            SuggestedStartLocation = PickerLocationId.VideosLibrary,
            SuggestedFileName = queue.Name.Length > 0 ? queue.Name : L("Playlist", "Плейлист"),
        };
        picker.FileTypeChoices.Add(L("Playlist", "Плейлист"), new List<string> { ".m3u8" });
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        var file = await picker.PickSaveFileAsync();
        if (file != null) player.Send("queue-save " + file.Path);
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        if (e.DataView.Contains(StandardDataFormats.StorageItems))
            e.AcceptedOperation = DataPackageOperation.Copy;
    }

    private async void OnDrop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems)) return;
        var items = await e.DataView.GetStorageItemsAsync();
        if (items.Count > 0) player.Send("open " + string.Join("\n", items.Select(i => i.Path)));
    }

    private void AddKeys()
    {
        void Key(VirtualKey key, Action action, VirtualKeyModifiers modifiers = VirtualKeyModifiers.None)
        {
            var accelerator = new KeyboardAccelerator { Key = key, Modifiers = modifiers };
            accelerator.Invoked += (_, args) =>
            {
                lastPointer = DateTime.UtcNow;
                action();
                args.Handled = true;
            };
            Root.KeyboardAccelerators.Add(accelerator);
        }

        Key(VirtualKey.Space, PlayOrPause);
        Key(VirtualKey.Left, () => player.Send("seek-by -5"));
        Key(VirtualKey.Right, () => player.Send("seek-by 5"));
        Key(VirtualKey.Left, () => player.Send("seek-by -60"), VirtualKeyModifiers.Shift);
        Key(VirtualKey.Right, () => player.Send("seek-by 60"), VirtualKeyModifiers.Shift);
        Key(VirtualKey.Up, () => player.Send("volume-by 0.05"));
        Key(VirtualKey.Down, () => player.Send("volume-by -0.05"));
        Key(VirtualKey.M, () => player.Send("toggle-mute"));
        Key(VirtualKey.S, NextSubtitles);
        Key(VirtualKey.A, NextSound);
        Key(VirtualKey.N, () => player.Send("next"));
        Key(VirtualKey.P, () => player.Send("previous"));
        Key(VirtualKey.L, () => { if (state.Queue != null) queueFlyout.ShowAt(QueueButton); });
        Key(VirtualKey.F, ToggleFullscreen);
        Key(VirtualKey.Enter, ToggleFullscreen);
        Key(VirtualKey.Escape, () => { if (IsFullscreen) ToggleFullscreen(); });
        Key(VirtualKey.I, () => { if (SignalPill.Visibility == Visibility.Visible) signalFlyout.ShowAt(SignalButton); });
        Key(VirtualKey.O, () => _ = ChooseFiles(adding: false), VirtualKeyModifiers.Control);
        Key((VirtualKey)188 /* comma */, () => settingsFlyout.ShowAt(SettingsButton), VirtualKeyModifiers.Control);
    }

    /// <summary>Step to the next subtitle track, ending with none.</summary>
    private void NextSubtitles()
    {
        var ids = state.SubtitleTracks.Select(t => t.Id).ToList();
        if (ids.Count == 0) return;
        var at = state.SubtitleTrack is int now ? ids.IndexOf(now) : -1;
        player.Send(at + 1 < ids.Count ? $"subtitle-track {ids[at + 1]}" : "subtitle-track off");
    }

    /// <summary>Step to the next sound track, going round.</summary>
    private void NextSound()
    {
        var ids = state.AudioTracks.Select(t => t.Id).ToList();
        if (ids.Count < 2 || state.AudioTrack is not int now) return;
        player.Send($"audio-track {ids[(ids.IndexOf(now) + 1) % ids.Count]}");
    }

    // ---- Track menus -------------------------------------------------------------------------

    private void FillSubtitlesMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        menu.Items.Add(Choice(L("Off", "Выключены"), "subtitles", state.SubtitleTrack == null, "subtitle-track off"));
        menu.Items.Add(new MenuFlyoutSeparator());
        foreach (var track in state.SubtitleTracks)
            menu.Items.Add(Choice(SubtitleLabel(track), "subtitles", state.SubtitleTrack == track.Id, $"subtitle-track {track.Id}"));
    }

    private void FillSoundMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        foreach (var track in state.AudioTracks)
            menu.Items.Add(Choice(SoundLabel(track), "sound", state.AudioTrack == track.Id, $"audio-track {track.Id}"));
    }

    private RadioMenuFlyoutItem Choice(string text, string group, bool on, string command)
    {
        var item = new RadioMenuFlyoutItem { Text = text, GroupName = group, IsChecked = on };
        item.Click += (_, _) => player.Send(command);
        return item;
    }
}

/// <summary>Shows a position on the seek bar as a time.</summary>
internal sealed class ClockConverter : IValueConverter
{
    private readonly Func<double> duration;

    public ClockConverter(Func<double> duration) => this.duration = duration;

    public object Convert(object value, Type targetType, object parameter, string language) =>
        Words.Clock(System.Convert.ToDouble(value) * duration());

    public object ConvertBack(object value, Type targetType, object parameter, string language) =>
        throw new NotSupportedException();
}
