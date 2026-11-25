using System.Runtime.InteropServices;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;
using Windows.Media;
using Windows.Media.Playback;
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
        Tip(ChaptersButton, L("Chapters (Page Up, Page Down)", "Главы (Page Up, Page Down)"));
        // Drawn, not a glyph: it has no text of its own for a screen reader to say.
        AutomationProperties.SetName(ChaptersButton, L("Chapters", "Главы"));
        Tip(SubtitlesButton, L("Subtitles", "Субтитры"));
        Tip(SpeedButton, L("Speed ([ and ])", "Скорость ([ и ])"));
        Tip(PictureButton, L("Picture", "Картинка"));
        AutomationProperties.SetName(PictureButton, L("Picture", "Картинка"));
        Tip(SoundButton, L("Sound track", "Звуковая дорожка"));

        // The play key and the played part of the seek bar take the signal's colour.
        PlayButton.Foreground = new SolidColorBrush(Slate);
        foreach (var key in new[] { "ButtonBackground", "ButtonBackgroundPointerOver", "ButtonBackgroundPressed" })
        {
            PlayButton.Resources[key] = live;
            MiniPlay.Resources[key] = live;
        }
        foreach (var key in new[] { "ButtonForeground", "ButtonForegroundPointerOver", "ButtonForegroundPressed" })
            MiniPlay.Resources[key] = new SolidColorBrush(Slate);
        Tip(MiniBack, L("Back to the big window (Ctrl+Shift+P)", "Обратно в большое окно (Ctrl+Shift+P)"));
        foreach (var key in new[] { "ButtonForeground", "ButtonForegroundPointerOver", "ButtonForegroundPressed" })
            PlayButton.Resources[key] = new SolidColorBrush(Slate);
        foreach (var key in new[] { "SliderTrackValueFill", "SliderTrackValueFillPointerOver", "SliderTrackValueFillPressed" })
            Seek.Resources[key] = live;
        Seek.ThumbToolTipValueConverter = new ClockConverter(() => state);
        ChapterMarks.SizeChanged += (_, _) => DrawChapterMarks(state);
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
        var speedMenu = new MenuFlyout();
        speedMenu.Opening += (_, _) => FillSpeedMenu(speedMenu);
        SpeedButton.Flyout = speedMenu;
        var pictureMenu = new MenuFlyout();
        pictureMenu.Opening += (_, _) => FillPictureMenu(pictureMenu);
        PictureButton.Flyout = pictureMenu;
        // The small picture over the seek bar follows the pointer.
        Seek.AddHandler(UIElement.PointerMovedEvent, new PointerEventHandler((_, e) => PointAt(e.GetCurrentPoint(Seek).Position.X)), true);
        Seek.PointerExited += (_, _) => { pointing = null; PreviewPane.Visibility = Visibility.Collapsed; };
        var chaptersMenu = new MenuFlyout();
        chaptersMenu.Opening += (_, _) => FillChaptersMenu(chaptersMenu);
        ChaptersButton.Flyout = chaptersMenu;
        foreach (var flyout in new FlyoutBase[] { signalFlyout, settingsFlyout, queueFlyout, subtitlesMenu, soundMenu, chaptersMenu, speedMenu, pictureMenu })
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

        SetUpMediaKeys();
        Video.SizeChanged += (_, _) => ResizeVideo();
        Video.CompositionScaleChanged += (_, _) => ResizeVideo();
        AddKeys();

        tick.Tick += (_, _) => OnTick();
        tick.Start();
        Closed += (_, _) =>
        {
            closed = true;
            tick.Stop();
            mediaKeysOwner?.Dispose();
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
            case "mini" when state.Signal != null:
                ToggleMini();
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

    /// <summary>Windows' own small window above everything: the player as a picture in a picture.</summary>
    private bool IsMini => AppWindow.Presenter.Kind == AppWindowPresenterKind.CompactOverlay;
    private bool onTop;

    private void ToggleMini()
    {
        AppWindow.SetPresenter(IsMini ? AppWindowPresenterKind.Default : AppWindowPresenterKind.CompactOverlay);
        if (!IsMini) KeepOnTop(onTop);
        controlsVisible = !controlsVisible; // shown again in the new arrangement
        KeepOrHideControls();
        Apply(state);
    }

    private void KeepOnTop(bool wanted)
    {
        onTop = wanted;
        if (AppWindow.Presenter is OverlappedPresenter overlapped) overlapped.IsAlwaysOnTop = wanted;
    }

    private void OnMiniClick(object sender, RoutedEventArgs e) => ToggleMini();

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
        if (KeyNote.Visibility == Visibility.Visible && DateTime.UtcNow > noteUntil) KeyNote.Visibility = Visibility.Collapsed;
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
        UpdateMediaKeys(s);
        // Changed while the same file plays: by a key, a menu, the system's controls.
        if (applied is PlayerState before && s.File != null && s.File == before.File)
        {
            if (s.Speed != before.Speed) Say(SpeedNote(s.Speed));
            if (s.SubtitleDelay != before.SubtitleDelay) Say(SubtitleDelayNote(s.SubtitleDelay));
            if (s.Settings.NightMode != before.Settings.NightMode)
                Say(s.Settings.NightMode ? L("Night sound on", "Ночной звук включён") : L("Night sound off", "Ночной звук выключен"));
            if (s.SavedFrames != before.SavedFrames)
                Say(s.SavedFrameError == null
                    ? L("Frame saved to Pictures", "Кадр сохранён в «Изображения»")
                    : L("The frame could not be saved", "Кадр не сохранился"));
        }
        var open = s.File != null;
        Empty.Visibility = open ? Visibility.Collapsed : Visibility.Visible;
        Chrome.Visibility = open && !IsMini ? Visibility.Visible : Visibility.Collapsed;
        MiniControls.Visibility = open && IsMini ? Visibility.Visible : Visibility.Collapsed;
        AppWindow.Title = open ? $"{s.File} - BitSpring Video" : "BitSpring Video";

        EmptyDisplay.Text = DisplayLine(s.Display);
        EmptyDisplay.Foreground = s.Display.Hdr == "off" ? Resource("BrassBrush") : Resource("QuietBrush");
        EmptyError.Text = s.Error ?? "";
        EmptyError.Visibility = s.Error == null ? Visibility.Collapsed : Visibility.Visible;
        ShowRecent(s);
        ResumeNote.Visibility = open && s.ResumedFrom != null && KeyNote.Visibility != Visibility.Visible
            ? Visibility.Visible : Visibility.Collapsed;
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
        MiniPlayGlyph.Glyph = PlayGlyph.Glyph;
        MiniProgress.Fill = live;
        MiniProgress.Width = s.Duration > 0 && Root.ActualWidth > 0 ? Root.ActualWidth * Math.Clamp(s.Position / s.Duration, 0, 1) : 0;
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

        ShowPreview(s);
        SpeedText.Text = SpeedName(s.Speed);
        SpeedText.Foreground = s.Speed == 1 ? Resource("QuietBrush") : Resource("PorcelainBrush");
        AutomationProperties.SetName(SpeedButton, SpeedNote(s.Speed));
        ChaptersButton.Visibility = s.Chapters.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        DrawChapterMarks(s);
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
        // Small, the window holds only what a glance needs.
        var big = !IsMini;
        Chrome.Visibility = big && state.File != null ? Visibility.Visible : Visibility.Collapsed;
        SettingsPill.Visibility = big ? Visibility.Visible : Visibility.Collapsed;
        MiniControls.Visibility = big || state.File == null ? Visibility.Collapsed : Visibility.Visible;
        Chrome.Opacity = keep ? 1 : 0;
        Chrome.IsHitTestVisible = keep;
        SettingsPill.Opacity = keep ? 1 : 0;
        SettingsPill.IsHitTestVisible = keep;
        MiniPlay.Opacity = keep ? 1 : 0;
        MiniPlay.IsHitTestVisible = keep;
        MiniBack.Opacity = keep ? 1 : 0;
        MiniBack.IsHitTestVisible = keep;
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
        Key(VirtualKey.PageDown, () => player.Send("chapter-next"));
        Key(VirtualKey.PageUp, () => player.Send("chapter-previous"));
        // Ctrl with an arrow, as it jumps a word in text, jumps a chapter here.
        Key(VirtualKey.Right, () => player.Send("chapter-next"), VirtualKeyModifiers.Control);
        Key(VirtualKey.Left, () => player.Send("chapter-previous"), VirtualKeyModifiers.Control);
        Key((VirtualKey)219 /* [ */, () => player.Send("speed-by -0.25"));
        Key((VirtualKey)221 /* ] */, () => player.Send("speed-by 0.25"));
        Key(VirtualKey.Back, () => player.Send("speed 1"));
        Key(VirtualKey.Z, () => player.Send("subtitle-delay-by -0.1"));
        Key(VirtualKey.X, () => player.Send("subtitle-delay-by 0.1"));
        Key(VirtualKey.Z, () => player.Send("subtitle-delay-by -1"), VirtualKeyModifiers.Shift);
        Key(VirtualKey.X, () => player.Send("subtitle-delay-by 1"), VirtualKeyModifiers.Shift);
        Key((VirtualKey)188 /* , */, () => player.Send("step -1"));
        Key((VirtualKey)190 /* . */, () => player.Send("step 1"));
        Key(VirtualKey.S, () => player.Send("save-frame"), VirtualKeyModifiers.Control);
        Key(VirtualKey.P, ToggleMini, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift);
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
        // Moved to meet the speech when they come too early or too late.
        if (state.SubtitleTrack == null) return;
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(new MenuFlyoutItem { Text = SubtitleDelayNote(state.SubtitleDelay), IsEnabled = false });
        menu.Items.Add(Action(L("Earlier by 0.1 s", "Раньше на 0.1 с"), "Z", "subtitle-delay-by -0.1"));
        menu.Items.Add(Action(L("Later by 0.1 s", "Позже на 0.1 с"), "X", "subtitle-delay-by 0.1"));
        if (state.SubtitleDelay != 0)
            menu.Items.Add(Action(L("On time", "Без сдвига"), "", "subtitle-delay 0"));
    }

    private static readonly double[] Speeds = { 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2 };
    private static readonly double[] Shapes = { 16.0 / 9, 4.0 / 3, 2.39, 1.85 };

    private static string ShapeName(double shape) =>
        Math.Abs(shape - 16.0 / 9) < 0.005 ? "16:9" : Math.Abs(shape - 4.0 / 3) < 0.005 ? "4:3" : Trimmed(shape, 2) + ":1";

    private void FillPictureMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        var p = state.Picture;
        bool Is(double value, double shape) => Math.Abs(value - shape) < 0.005;
        menu.Items.Add(new MenuFlyoutItem { Text = L("Shape", "Форма"), IsEnabled = false });
        menu.Items.Add(Choice(L("As made", "Как в файле"), "shape", p.Shape == 0, "picture-shape 0"));
        foreach (var shape in Shapes)
            menu.Items.Add(Choice(ShapeName(shape), "shape", Is(p.Shape, shape), "picture-shape " + Number(shape)));
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(new MenuFlyoutItem { Text = L("Cut away bars", "Обрезать полосы"), IsEnabled = false });
        menu.Items.Add(Choice(L("None", "Нет"), "cut", p.Cut == 0, "picture-cut 0"));
        foreach (var shape in Shapes)
            menu.Items.Add(Choice(L($"To {ShapeName(shape)}", $"До {ShapeName(shape)}"), "cut", Is(p.Cut, shape), "picture-cut " + Number(shape)));
        menu.Items.Add(new MenuFlyoutSeparator());
        var fill = new ToggleMenuFlyoutItem { Text = L("Fill the window", "Заполнить окно"), IsChecked = p.Fill };
        fill.Click += (_, _) => player.Send(p.Fill ? "picture-fill 0" : "picture-fill 1");
        menu.Items.Add(fill);
        menu.Items.Add(new MenuFlyoutSeparator());
        var top = new ToggleMenuFlyoutItem { Text = L("Keep above other windows", "Поверх других окон"), IsChecked = onTop };
        top.Click += (_, _) => KeepOnTop(!onTop);
        menu.Items.Add(top);
        var mini = new MenuFlyoutItem { Text = L("Small, in a corner", "Маленьким, в углу"), KeyboardAcceleratorTextOverride = "Ctrl+Shift+P" };
        mini.Click += (_, _) => ToggleMini();
        menu.Items.Add(mini);
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(Action(L("Save this frame", "Сохранить кадр"), "Ctrl+S", "save-frame"));
    }

    // ---- Files played lately -------------------------------------------------------------------

    private string recentShown = "";

    private void ShowRecent(PlayerState s)
    {
        var key = string.Join("|", s.Recent.Select(r => r.Path + r.Position));
        if (key == recentShown) return;
        recentShown = key;
        RecentList.Children.Clear();
        RecentList.Visibility = s.Recent.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        if (s.Recent.Count == 0) return;
        RecentList.Children.Add(new TextBlock
        {
            Text = L("Played lately", "Недавно смотрели"),
            FontSize = 12,
            Margin = new Thickness(12, 0, 0, 4),
            Foreground = Resource("QuietBrush"),
        });
        foreach (var file in s.Recent)
        {
            var row = new Grid { Height = 30, Padding = new Thickness(12, 0, 12, 0) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var name = new TextBlock
            {
                Text = file.Name,
                FontSize = 13,
                FontWeight = Microsoft.UI.Text.FontWeights.Medium,
                TextTrimming = TextTrimming.CharacterEllipsis,
                VerticalAlignment = VerticalAlignment.Center,
                Foreground = Resource("PorcelainBrush"),
            };
            var where = new TextBlock
            {
                Text = file.Position > 0
                    ? L($"stopped at {Clock(file.Position)}", $"остановлено на {Clock(file.Position)}")
                    : file.Duration > 0 ? L("watched", "досмотрено") : "",
                FontSize = 12,
                Margin = new Thickness(16, 0, 0, 0),
                VerticalAlignment = VerticalAlignment.Center,
                Foreground = Resource("QuietBrush"),
            };
            Grid.SetColumn(where, 1);
            row.Children.Add(name);
            row.Children.Add(where);
            var button = new Button
            {
                Content = row,
                Style = (Style)Root.Resources["OnGlass"],
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
                Padding = new Thickness(0),
            };
            var path = file.Path;
            button.Click += (_, _) => player.Send("open " + path);
            ToolTipService.SetToolTip(button, path);
            RecentList.Children.Add(button);
        }
    }

    // ---- The small picture over the seek bar ---------------------------------------------------

    private double? pointing;
    private double thumbnailAsked = -1;
    private DateTime thumbnailAskedAt = DateTime.MinValue;
    private string previewShown = "";

    /// <summary>The pointer is over the seek bar, `x` from its left edge.</summary>
    private void PointAt(double x)
    {
        if (state.Duration <= 0 || Seek.ActualWidth <= 0) return;
        var fraction = Math.Clamp(x / Seek.ActualWidth, 0, 1);
        var seconds = fraction * state.Duration;
        pointing = seconds;
        var step = Math.Max(state.Duration / 300, 1);
        if (Math.Abs(seconds - thumbnailAsked) >= step / 2 && (DateTime.UtcNow - thumbnailAskedAt).TotalMilliseconds >= 80)
        {
            thumbnailAsked = seconds;
            thumbnailAskedAt = DateTime.UtcNow;
            player.Send("thumbnail " + Number(seconds));
        }
        // Centred over the pointer, standing on the transport bar.
        var at = Seek.TransformToVisual(Root).TransformPoint(new Windows.Foundation.Point(x, 0));
        PreviewPane.Margin = new Thickness(Math.Max(8, at.X - 88), 0, 0, Root.ActualHeight - at.Y + 26);
        PreviewTime.Text = state.ChapterAt(seconds) is int n ? $"{Clock(seconds)}  {ChapterName(state, n)}" : Clock(seconds);
        ShowPreview(state);
    }

    private void ShowPreview(PlayerState s)
    {
        if (pointing == null || s.Thumbnail == null)
        {
            if (pointing == null) PreviewPane.Visibility = Visibility.Collapsed;
            return;
        }
        if (s.Thumbnail.Path != previewShown)
        {
            previewShown = s.Thumbnail.Path;
            PreviewImage.Source = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(s.Thumbnail.Path));
        }
        PreviewPane.Visibility = Visibility.Visible;
    }

    private void FillSpeedMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        foreach (var speed in Speeds)
            menu.Items.Add(Choice(speed == 1 ? L("Normal", "Обычная") : SpeedName(speed), "speed", state.Speed == speed, "speed " + Number(speed)));
    }

    private MenuFlyoutItem Action(string text, string key, string command)
    {
        var item = new MenuFlyoutItem { Text = text, KeyboardAcceleratorTextOverride = key };
        item.Click += (_, _) => player.Send(command);
        return item;
    }

    // ---- A word about what a key just changed --------------------------------------------------

    private DateTime noteUntil = DateTime.MinValue;

    private void Say(string text)
    {
        KeyNoteText.Text = text;
        KeyNote.Visibility = Visibility.Visible;
        ResumeNote.Visibility = Visibility.Collapsed;
        noteUntil = DateTime.UtcNow.AddSeconds(1.6);
    }

    private void FillSoundMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        foreach (var track in state.AudioTracks)
            menu.Items.Add(Choice(SoundLabel(track), "sound", state.AudioTrack == track.Id, $"audio-track {track.Id}"));
    }

    /// <summary>Each chapter by its name, with where it begins on the right; the one playing ticked.</summary>
    private void FillChaptersMenu(MenuFlyout menu)
    {
        menu.Items.Clear();
        for (var n = 0; n < state.Chapters.Count; n++)
        {
            var item = Choice(ChapterName(state, n), "chapters", state.Chapter == n, "seek-to " + Number(state.Chapters[n].Start));
            item.KeyboardAcceleratorTextOverride = Clock(state.Chapters[n].Start);
            menu.Items.Add(item);
        }
    }

    // ---- Chapters on the seek bar --------------------------------------------------------------

    private string chapterMarksDrawn = "";

    /// <summary>A cut across the seek bar where each chapter after the first begins.</summary>
    private void DrawChapterMarks(PlayerState s)
    {
        var width = ChapterMarks.ActualWidth;
        var key = $"{width}|{s.Duration}|" + string.Join("|", s.Chapters.Select(c => Number(c.Start)));
        if (key == chapterMarksDrawn) return;
        chapterMarksDrawn = key;
        ChapterMarks.Children.Clear();
        if (s.Duration <= 0 || width <= 0) return;
        // The thumb's centre travels between half its width from either end.
        var thumb = Application.Current.Resources.TryGetValue("SliderHorizontalThumbWidth", out var w) && w is double d ? d : 18;
        foreach (var chapter in s.Chapters.Skip(1))
        {
            var x = thumb / 2 + chapter.Start / s.Duration * (width - thumb);
            var cut = new Rectangle { Width = 2, Height = ChapterMarks.Height, Fill = Transport.Background };
            Canvas.SetLeft(cut, Math.Round(x) - 1);
            ChapterMarks.Children.Add(cut);
        }
    }

    // ---- The system's media keys ---------------------------------------------------------------

    // Windows sends the play, next and previous keys (and the buttons in its media overlay)
    // to whichever program has told it what it is playing. A program of this kind tells it
    // through a media player of the system's own, which plays nothing here: it only lends
    // its connection to those keys.
    private MediaPlayer? mediaKeysOwner;
    private SystemMediaTransportControls? mediaKeys;
    private string mediaKeysShown = "";
    private DateTime mediaKeysTimeline = DateTime.MinValue;

    private void SetUpMediaKeys()
    {
        try
        {
            mediaKeysOwner = new MediaPlayer();
            mediaKeysOwner.CommandManager.IsEnabled = false;
            var keys = mediaKeysOwner.SystemMediaTransportControls;
            keys.IsPlayEnabled = true;
            keys.IsPauseEnabled = true;
            keys.IsNextEnabled = true;
            keys.IsPreviousEnabled = true;
            keys.ButtonPressed += (_, e) => DispatcherQueue.TryEnqueue(() => OnMediaKey(e.Button));
            keys.PlaybackPositionChangeRequested += (_, e) =>
                DispatcherQueue.TryEnqueue(() => player.Send("seek-to " + Number(e.RequestedPlaybackPosition.TotalSeconds)));
            mediaKeys = keys;
        }
        catch (Exception)
        {
            // Without them the player plays on; only the keys outside its window do nothing.
            mediaKeys = null;
        }
    }

    private void OnMediaKey(SystemMediaTransportControlsButton button)
    {
        if (closed || state.File == null) return;
        lastPointer = DateTime.UtcNow;
        switch (button)
        {
            case SystemMediaTransportControlsButton.Play when state.Paused || state.Ended:
            case SystemMediaTransportControlsButton.Pause when !state.Paused:
                PlayOrPause();
                break;
            // In a film a track is a chapter; after the last, the next file of the series.
            case SystemMediaTransportControlsButton.Next:
                player.Send("skip-next");
                break;
            case SystemMediaTransportControlsButton.Previous:
                player.Send("skip-previous");
                break;
        }
    }

    private void UpdateMediaKeys(PlayerState s)
    {
        if (mediaKeys == null) return;
        if (s.File == null)
        {
            if (mediaKeysShown.Length == 0) return;
            mediaKeysShown = "";
            mediaKeys.PlaybackStatus = MediaPlaybackStatus.Stopped;
            mediaKeys.DisplayUpdater.ClearAll();
            mediaKeys.DisplayUpdater.Update();
            return;
        }
        mediaKeys.IsEnabled = true;
        mediaKeys.PlaybackStatus = s.Paused || s.Ended ? MediaPlaybackStatus.Paused : MediaPlaybackStatus.Playing;
        // The line under the title: the chapter playing, or else the series the file is of.
        var subtitle = s.Chapter is int n ? ChapterName(s, n)
            : s.Queue is { Items.Count: > 1 } queue ? queue.Name : "";
        var shown = s.File + "|" + subtitle;
        if (shown != mediaKeysShown)
        {
            mediaKeysShown = shown;
            var display = mediaKeys.DisplayUpdater;
            display.Type = MediaPlaybackType.Video;
            display.VideoProperties.Title = System.IO.Path.GetFileNameWithoutExtension(s.File);
            display.VideoProperties.Subtitle = subtitle;
            display.Update();
        }
        // The overlay's own seek bar: kept up to date a few times a minute is enough.
        if ((DateTime.UtcNow - mediaKeysTimeline).TotalSeconds >= 5 || s.Paused)
        {
            mediaKeysTimeline = DateTime.UtcNow;
            mediaKeys.UpdateTimelineProperties(new SystemMediaTransportControlsTimelineProperties
            {
                StartTime = TimeSpan.Zero,
                MinSeekTime = TimeSpan.Zero,
                EndTime = TimeSpan.FromSeconds(s.Duration),
                MaxSeekTime = TimeSpan.FromSeconds(s.Duration),
                Position = TimeSpan.FromSeconds(s.Position),
            });
        }
    }

    private RadioMenuFlyoutItem Choice(string text, string group, bool on, string command)
    {
        var item = new RadioMenuFlyoutItem { Text = text, GroupName = group, IsChecked = on };
        item.Click += (_, _) => player.Send(command);
        return item;
    }
}

/// <summary>Shows a position on the seek bar as a time, and the chapter it falls in.</summary>
internal sealed class ClockConverter : IValueConverter
{
    private readonly Func<PlayerState> state;

    public ClockConverter(Func<PlayerState> state) => this.state = state;

    public object Convert(object value, Type targetType, object parameter, string language)
    {
        var s = state();
        var seconds = System.Convert.ToDouble(value) * s.Duration;
        return s.ChapterAt(seconds) is int n ? $"{Words.Clock(seconds)}  {Words.ChapterName(s, n)}" : Words.Clock(seconds);
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language) =>
        throw new NotSupportedException();
}
