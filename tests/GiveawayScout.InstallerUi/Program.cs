using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.Win32;

// This harness owns only the setup process that it starts. It never invokes the
// Install button, changes Windows language/DPI, or closes another application.
if (!OperatingSystem.IsWindows()) return 2;
Native.SetProcessDpiAwarenessContext(new IntPtr(-4)); // Per-monitor V2, this process only.
try
{
    var options = Options.Parse(args);
    Directory.CreateDirectory(options.Output);
    if (options.MeasureOnly)
    {
        var measurements = Probe.MeasureStockLayout();
        Probe.WriteJson(Path.Combine(options.Output, "measurements.json"), measurements);
        Console.WriteLine("PASS: GDI text/layout measurements at 96/144/192 DPI. No setup was launched; these are not real DPI UI tests.");
        return 0;
    }
    var result = new Probe(options).Run();
    Probe.WriteJson(Path.Combine(options.Output, "acceptance.json"), result);
    Console.WriteLine($"PASS: component page {result.Language}, real window DPI {result.ActualDpi}. No installation was started.");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(error.ToString());
    return error is ArgumentException ? 2 : 1;
}

record Options(string Workspace, string Setup, string Output, string Language, bool MeasureOnly)
{
    public static Options Parse(string[] args)
    {
        var values = new Dictionary<string, string>();
        bool measure = false;
        for (int i = 0; i < args.Length; i++)
        {
            if (args[i] == "--measure-only") { measure = true; continue; }
            if (args[i] is not ("--workspace" or "--setup" or "--output" or "--language") || ++i == args.Length)
                throw new ArgumentException("Usage: --workspace <root> --output <root/artifacts/...> [--measure-only | --setup <exe> --language auto|ru|en]");
            if (!values.TryAdd(args[i - 1], args[i])) throw new ArgumentException("Duplicate option.");
        }
        string workspace = Path.TrimEndingDirectorySeparator(Path.GetFullPath(values.GetValueOrDefault("--workspace") ?? Directory.GetCurrentDirectory()));
        if (!Directory.Exists(Path.Combine(workspace, "src", "GiveawayScout.Desktop")))
            throw new ArgumentException("Workspace must be the Giveaway Scout repository.");
        string output = OwnedPath(workspace, values.GetValueOrDefault("--output") ??
            throw new ArgumentException("An isolated --output under workspace/artifacts is required."));
        if (!output.StartsWith(Path.Combine(workspace, "artifacts") + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Output must be a child of workspace/artifacts.");
        if (Directory.Exists(output) && Directory.EnumerateFileSystemEntries(output).Any())
            throw new ArgumentException("Use a new, empty output directory for each run.");
        string setup = measure ? "" : OwnedPath(workspace, values.GetValueOrDefault("--setup") ??
            throw new ArgumentException("--setup is required unless --measure-only is used."));
        if (!measure && (!File.Exists(setup) || !setup.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)))
            throw new ArgumentException("Setup must be an existing workspace EXE.");
        string language = values.GetValueOrDefault("--language") ?? "auto";
        if (language is not ("auto" or "ru" or "en")) throw new ArgumentException("Invalid --language.");
        return new(workspace, setup, output, language, measure);
    }

    static string OwnedPath(string root, string value)
    {
        string path = Path.GetFullPath(value, root);
        if (!path.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Paths must remain inside the workspace.");
        for (var item = new DirectoryInfo(Directory.Exists(path) ? path : Path.GetDirectoryName(path)!); item is not null; item = item.Parent)
        {
            if (item.Exists && (item.Attributes & FileAttributes.ReparsePoint) != 0)
                throw new ArgumentException("Reparse-point paths are not allowed for the test.");
            if (item.FullName.Equals(root, StringComparison.OrdinalIgnoreCase)) break;
        }
        if (File.Exists(path) && (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new ArgumentException("Reparse-point files are not allowed for the test.");
        return path;
    }
}

record ControlSnapshot(int Id, string Class, string Text, Native.Rect Bounds);
record TreeItem(IntPtr Handle, string Text, int CheckState, Native.Rect Bounds);
record AcceptanceResult(string Language, uint ActualDpi, int ProcessId, string SetupSha256,
    object[] Checks, ControlSnapshot[] Controls, object[] ScaleMeasurements,
    bool RegistryUnchanged, bool InstallInvoked, bool WindowsSettingsChanged);

sealed class Probe(Options options)
{
    Process? process;
    IntPtr window;
    readonly List<object> checks = [];
    string Language = "";
    string RegistryBefore = "";

    public AcceptanceResult Run()
    {
        RegistryBefore = RegistryFingerprint();
        string temporary = Path.Combine(options.Output, "setup-temp");
        Directory.CreateDirectory(temporary);
        string target = Path.Combine(options.Output, "installation-must-not-exist");
        var info = new ProcessStartInfo(options.Setup)
        {
            // NSIS /D= must be last and unquoted, including paths with spaces.
            Arguments = "/D=" + target, UseShellExecute = false, WorkingDirectory = options.Output
        };
        info.Environment["TEMP"] = temporary;
        info.Environment["TMP"] = temporary;
        Native.GetCursorPos(out var cursor);
        IntPtr previousForeground = Native.GetForegroundWindow();
        try
        {
            process = Process.Start(info) ?? throw new InvalidOperationException("Setup did not start.");
            window = WaitFor(() =>
            {
                var owned = OwnedWindows();
                var chooser = owned.FirstOrDefault(w => Native.Class(w) == "#32770" &&
                    Children(w).Any(c => Native.Class(c) == "ComboBox") && Native.Text(w) == "Installer Language");
                if (chooser != IntPtr.Zero)
                {
                    ChooseLanguage(chooser);
                    return IntPtr.Zero;
                }
                return owned.FirstOrDefault(w => Native.Class(w) == "#32770" && Native.GetDlgItem(w, 3) != IntPtr.Zero);
            }, "setup wizard");
            for (int page = 0; page < 6 && Tree() == IntPtr.Zero; page++)
            {
                var edits = VisibleControls().Where(c => c.Class == "Edit").ToArray();
                if (edits.Any(c => c.Text.Contains(':') || c.Text.Contains('\\')))
                    Check(edits.Any(c => Path.GetFullPath(c.Text).Equals(target, StringComparison.OrdinalIgnoreCase)),
                        "Directory page uses the isolated /D target");
                Next();
                Thread.Sleep(250);
            }
            IntPtr tree = Tree();
            Check(tree != IntPtr.Zero, "Reached component page without starting installation");
            var items = ReadItems(tree);
            WriteJson(Path.Combine(options.Output, "tree-items.json"), items.Select(item => new { item.Text, item.CheckState, item.Bounds }));
            Check(items.Length == 2, "Exactly two visible components");
            Language = items[0].Text.Contains("Графическое") ? "ru" : "en";
            if (options.Language != "auto") Check(Language == options.Language, "Requested installer language is active");
            bool ru = Language == "ru";
            string[] labels = ru ? ["Графическое приложение", "Консольное приложение"] : ["Graphical application", "Command-line application"];
            string[] descriptions = ru
                ? ["Окно, уведомления и значок в трее. Использует WebView2.", "Работа через терминал. Добавляет папку программы в PATH."]
                : ["Window, notifications and tray icon. Uses WebView2.", "Terminal commands. Adds the installation folder to PATH."];
            Check(items.Select(x => x.Text).SequenceEqual(labels), "Both component names are complete");
            var controls = VisibleControls();
            string instructions = Native.Text(Native.GetDlgItem(CurrentPage(), 1006));
            Check(!ru || instructions.Contains("«Далее»"), "Russian instructions use guillemets");
            Check(!HorizontalScrollVisible(tree), "Component tree has no visible horizontal scrollbar");
            var client = Native.Client(tree);
            Check(items.All(item => item.Bounds.Right <= client.Right), "Every component label fits within the tree viewport");
            var pageWindow = CurrentPage();
            IntPtr description = Native.GetDlgItem(pageWindow, 1043);
            string placeholder = ru ? "Наведите курсор на компонент, чтобы увидеть описание." : "Hover over a component to see its description.";
            // Windows may reject SetForegroundWindow after rapid test launches.
            // Raise only our temporary window, then activate it via inert text.
            Native.SetWindowPos(window, new IntPtr(-1), 0, 0, 0, 0, 0x43);
            Native.SetForegroundWindow(window);
            // Always establish a mouse-out state before testing entry into rows.
            Hover(Native.GetDlgItem(pageWindow, 1006), Native.Client(Native.GetDlgItem(pageWindow, 1006)));
            if (Native.GetForegroundWindow() != window)
            {
                Native.MouseEvent(2, 0, 0, 0, UIntPtr.Zero);
                Native.MouseEvent(4, 0, 0, 0, UIntPtr.Zero);
                WaitUntil(() => Native.GetForegroundWindow() == window, "owned setup activation");
            }
            Check(Native.GetForegroundWindow() == window, "Owned setup is foreground before mouse hover checks");
            WaitUntil(() => Native.Text(description) == placeholder, "initial description placeholder");
            Check(!string.IsNullOrWhiteSpace(placeholder), "Description placeholder is present");
            controls = VisibleControls();
            string placeholderImage = Path.Combine(options.Output, "components-placeholder.bmp");
            Native.SaveWindowBmp(window, placeholderImage);
            CheckDescriptionContrast(placeholderImage, description, "placeholder");
            for (int i = 0; i < items.Length; i++)
            {
                Hover(tree, items[i].Bounds);
                WaitUntil(() => Native.Text(description) == descriptions[i], $"hover description {i + 1}");
                checks.Add(new { description = $"Hover changes description for {labels[i]}", passed = true,
                    actualText = Native.Text(description), bounds = Native.Bounds(description) });
                string hoverImage = Path.Combine(options.Output, $"components-hover-{(i == 0 ? "gui" : "cli")}.bmp");
                Native.SaveWindowBmp(window, hoverImage);
                CheckDescriptionContrast(hoverImage, description, labels[i]);
            }
            Hover(Native.GetDlgItem(pageWindow, 1006), Native.Client(Native.GetDlgItem(pageWindow, 1006)));
            WaitUntil(() => Native.Text(description) == placeholder, "description placeholder after leaving tree");
            Check(true, "Leaving component tree restores the placeholder");
            Native.SaveWindowBmp(window, Path.Combine(options.Output, "components.bmp"));
            var before = items.Select(i => i.CheckState).ToArray();
            bool targetGui = before[0] != 2;
            SetChecked(tree, items[1], true);
            SetChecked(tree, items[0], targetGui); // Always changes the GUI selection; never empty.
            ClickButton(3, "back");
            Thread.Sleep(250);
            Next();
            WaitUntil(() => Tree() != IntPtr.Zero, "component page after back/next");
            var restored = ReadItems(Tree());
            Check(restored[0].CheckState == (targetGui ? 2 : 1) && restored[1].CheckState == 2,
                "Back/Next preserves an explicitly changed component selection");
            // Restore the original selection before cancelling the test process.
            for (int i = 0; i < restored.Length; i++) if (before[i] == 2) SetChecked(Tree(), restored[i], true);
            for (int i = 0; i < restored.Length; i++) if (before[i] != 2) SetChecked(Tree(), restored[i], false);
            Check(!Directory.Exists(target), "No installation destination was created");
            // Back/Next rebuilds the native page and invalidates its old HWNDs.
            tree = Tree();
            description = Native.GetDlgItem(Native.GetParent(tree), 1043);
            var measurements = MeasureLayout(tree, description, labels.Concat(descriptions).Append(placeholder).ToArray());
            uint dpi = Native.GetDpiForWindow(window);
            Cancel();
            Check(RegistryBefore == RegistryFingerprint(), "Installation registration and user PATH did not change");
            return new(Language, dpi, process.Id, Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(options.Setup))),
                checks.ToArray(), controls, measurements, true, false, false);
        }
        catch (Exception error)
        {
            try
            {
                if (window != IntPtr.Zero && Native.IsWindow(window))
                    Native.SaveWindowBmp(window, Path.Combine(options.Output, "failure.bmp"));
                WriteJson(Path.Combine(options.Output, "failure.json"), new { error = error.ToString(), checks, controls = window == IntPtr.Zero ? [] : VisibleControls(), installInvoked = false });
            }
            catch (Exception captureError) { Console.Error.WriteLine("Failure metadata capture: " + captureError.Message); }
            throw;
        }
        finally
        {
            try { if (process is { HasExited: false }) Cancel(); }
            finally
            {
                Native.SetCursorPos(cursor.X, cursor.Y);
                if (previousForeground != IntPtr.Zero && Native.IsWindow(previousForeground)) Native.SetForegroundWindow(previousForeground);
                process?.Dispose();
            }
        }
    }

    void ChooseLanguage(IntPtr chooser)
    {
        var combo = Children(chooser).Single(c => Native.Class(c) == "ComboBox");
        if (options.Language != "auto")
        {
            int count = (int)Native.Send(combo, 0x146, IntPtr.Zero, IntPtr.Zero); // CB_GETCOUNT
            var desired = options.Language == "ru" ? "Русский" : "English";
            int selected = -1;
            for (int i = 0; i < count; i++)
            {
                var value = new StringBuilder(256);
                Native.SendText(combo, 0x148, new IntPtr(i), value); // CB_GETLBTEXT
                if (value.ToString().Contains(desired, StringComparison.OrdinalIgnoreCase)) selected = i;
            }
            Check(selected >= 0, "Requested language exists in the test-only language chooser");
            Native.Send(combo, 0x14E, new IntPtr(selected), IntPtr.Zero); // CB_SETCURSEL
        }
        // This OK belongs specifically to the verified language dialog, never an Install page.
        Native.Send(Native.GetDlgItem(chooser, 1), 0xF5, IntPtr.Zero, IntPtr.Zero);
    }

    void Next()
    {
        string caption = Native.Text(Native.GetDlgItem(window, 1)).Replace("&", "").Trim();
        Check(caption is "Next >" or "Далее >", $"Safe Next button caption: {caption}");
        ClickButton(1, "next");
    }
    void ClickButton(int id, string name)
    {
        if (id is not (1 or 3)) throw new InvalidOperationException("Only Next/Back navigation is permitted.");
        IntPtr button = Native.GetDlgItem(window, id);
        if (button == IntPtr.Zero || !Native.IsWindowEnabled(button)) throw new InvalidOperationException($"Missing {name} button.");
        if (id == 1 && Native.Text(button).Replace("&", "").Trim() is not ("Next >" or "Далее >"))
            throw new InvalidOperationException("Refusing to invoke an Install or unknown button.");
        Native.Send(button, 0xF5, IntPtr.Zero, IntPtr.Zero);
    }
    void Cancel()
    {
        if (process is null || process.HasExited) return;
        foreach (var owned in OwnedWindows()) Native.PostMessage(owned, 0x10, IntPtr.Zero, IntPtr.Zero);
        var timeout = Stopwatch.StartNew();
        while (!process.HasExited && timeout.ElapsedMilliseconds < 5000)
        {
            // Only answer Yes on an owned abort confirmation; never touch other windows.
            foreach (var owned in OwnedWindows())
            {
                if (Native.GetDlgItem(owned, 6) != IntPtr.Zero && Native.GetDlgItem(owned, 7) != IntPtr.Zero)
                    Native.Send(Native.GetDlgItem(owned, 6), 0xF5, IntPtr.Zero, IntPtr.Zero);
            }
            Thread.Sleep(100);
        }
        if (!process.HasExited) throw new InvalidOperationException($"Owned setup PID {process.Id} did not cancel. No application process was killed.");
    }

    IntPtr[] OwnedWindows()
    {
        var result = new List<IntPtr>();
        Native.EnumWindows((h, _) => { Native.GetWindowThreadProcessId(h, out uint pid); if (pid == process!.Id && Native.IsWindowVisible(h)) result.Add(h); return true; }, IntPtr.Zero);
        return result.ToArray();
    }
    static IntPtr[] Children(IntPtr parent)
    {
        var result = new List<IntPtr>();
        Native.EnumChildWindows(parent, (h, _) => { result.Add(h); return true; }, IntPtr.Zero);
        return result.ToArray();
    }
    IntPtr Tree() => Children(window).FirstOrDefault(h => Native.Class(h) == "SysTreeView32" && Native.IsWindowVisible(h));
    IntPtr CurrentPage() => Native.GetParent(Tree());
    ControlSnapshot[] VisibleControls() => Children(window).Where(Native.IsWindowVisible)
        .Select(h => new ControlSnapshot(Native.GetDlgCtrlID(h), Native.Class(h), Native.Text(h), Native.Bounds(h))).ToArray();
    void Check(bool condition, string description)
    {
        if (!condition) throw new InvalidOperationException(description);
        checks.Add(new { description, passed = true });
    }
    void CheckDescriptionContrast(string image, IntPtr description, string state)
    {
        byte[] data = File.ReadAllBytes(image);
        int width = BitConverter.ToInt32(data, 18), height = BitConverter.ToInt32(data, 22);
        var outer = Native.Bounds(window);
        var rect = Native.Bounds(description);
        var colors = new Dictionary<int, int>();
        for (int y = rect.Top - outer.Top; y < rect.Bottom - outer.Top; y++)
        for (int x = rect.Left - outer.Left; x < rect.Right - outer.Left; x++)
        {
            if (x < 0 || x >= width || y < 0 || y >= height) throw new InvalidOperationException("Description capture lies outside the owned window.");
            int offset = 54 + ((height - 1 - y) * width + x) * 4;
            int color = data[offset + 2] << 16 | data[offset + 1] << 8 | data[offset];
            colors[color] = colors.GetValueOrDefault(color) + 1;
        }
        static double Luminance(int color)
        {
            var channels = new[] { (color >> 16) & 255, (color >> 8) & 255, color & 255 }
                .Select(value => value / 255d).Select(value => value <= 0.04045 ? value / 12.92 : Math.Pow((value + 0.055) / 1.055, 2.4)).ToArray();
            return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
        }
        int background = colors.MaxBy(pair => pair.Value).Key;
        int foreground = colors.OrderBy(pair => Luminance(pair.Key)).First().Key;
        double ratio = (Luminance(background) + 0.05) / (Luminance(foreground) + 0.05);
        if (ratio < 4.5) throw new InvalidOperationException($"Description {state} contrast is only {ratio:F2}:1.");
        if (state != "placeholder" && foreground != 0)
            throw new InvalidOperationException($"Description {state} did not immediately repaint with black text.");
        checks.Add(new { description = $"Description {state} has readable rendered contrast", passed = true,
            foreground = foreground.ToString("X6"), background = background.ToString("X6"), contrastRatio = ratio,
            evidence = "Darkest rendered text pixel against the dominant background in the owned Static rectangle" });
    }
    static IntPtr WaitFor(Func<IntPtr> read, string description)
    {
        var timeout = Stopwatch.StartNew();
        while (timeout.ElapsedMilliseconds < 15000) { var value = read(); if (value != IntPtr.Zero) return value; Thread.Sleep(100); }
        throw new TimeoutException("Timed out: " + description);
    }
    static void WaitUntil(Func<bool> read, string description) => WaitFor(() => read() ? new IntPtr(1) : IntPtr.Zero, description);
    static void Hover(IntPtr control, Native.Rect rect)
    {
        var point = new Native.Point { X = rect.Left + Math.Max(1, (rect.Right - rect.Left) / 2), Y = rect.Top + Math.Max(1, (rect.Bottom - rect.Top) / 2) };
        Native.ClientToScreen(control, ref point);
        if (!Native.SetCursorPos(point.X, point.Y)) throw new InvalidOperationException("Could not move cursor over owned setup.");
        Native.GetCursorPos(out var actual);
        IntPtr hit = Native.WindowFromPoint(point);
        Console.WriteLine($"Hover {Native.Class(control)} local [{rect.Left},{rect.Top},{rect.Right},{rect.Bottom}] screen [{point.X},{point.Y}] actual [{actual.X},{actual.Y}] hit {Native.Class(hit)}.");
        if (actual.X != point.X || actual.Y != point.Y || hit != control)
            throw new InvalidOperationException("Mouse did not reach the expected owned setup control.");
    }
    static bool HorizontalScrollVisible(IntPtr tree)
    {
        if ((Native.GetWindowLong(tree, -16) & 0x100000) == 0) return false;
        var bar = new Native.ScrollBarInfo { Size = (uint)Marshal.SizeOf<Native.ScrollBarInfo>(), State = new uint[6] };
        return !Native.GetScrollBarInfo(tree, -6, ref bar) || (bar.State[0] & 0x8000) == 0;
    }
    TreeItem[] ReadItems(IntPtr tree)
    {
        var result = new List<TreeItem>();
        using var memory = new RemoteMemory(process!.Id);
        IntPtr item = Native.Send(tree, 0x110A, IntPtr.Zero, IntPtr.Zero); // TVGN_ROOT
        while (item != IntPtr.Zero)
        {
            var (text, state) = memory.Item(tree, item);
            result.Add(new(item, text, state, memory.ItemRect(tree, item)));
            item = Native.Send(tree, 0x110A, new IntPtr(1), item); // TVGN_NEXT
        }
        return result.ToArray();
    }
    void SetChecked(IntPtr tree, TreeItem item, bool desired)
    {
        if (ReadItems(tree).Single(i => i.Handle == item.Handle).CheckState == (desired ? 2 : 1)) return;
        var checkbox = item.Bounds;
        checkbox.Right = checkbox.Left - 1;
        checkbox.Left = Math.Max(0, checkbox.Left - 18);
        Hover(tree, checkbox);
        Native.MouseEvent(2, 0, 0, 0, UIntPtr.Zero); // Physical mouse input reaches NSIS dialog preprocessing.
        Native.MouseEvent(4, 0, 0, 0, UIntPtr.Zero);
        WaitUntil(() => ReadItems(tree).Single(i => i.Handle == item.Handle).CheckState == (desired ? 2 : 1), "component selection");
    }

    static string RegistryFingerprint()
    {
        var text = new StringBuilder();
        foreach (string path in new[] { @"Software\giveawayscout\Giveaway Scout", @"Software\Microsoft\Windows\CurrentVersion\Uninstall\Giveaway Scout", @"Environment" })
        {
            using var key = Registry.CurrentUser.OpenSubKey(path);
            text.Append(path).Append(':');
            if (key is null) { text.Append("<missing>"); continue; }
            foreach (var name in key.GetValueNames().Order()) text.Append(name).Append('=').Append(key.GetValue(name)).Append(';');
        }
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(text.ToString())));
    }
    public static void WriteJson(string path, object value) => File.WriteAllText(path,
        JsonSerializer.Serialize(value, new JsonSerializerOptions { WriteIndented = true, IncludeFields = true }), new UTF8Encoding(false));

    public static object[] MeasureStockLayout() => MeasureStrings(new Native.LogFont
    { Height = -11, Weight = 400, CharSet = 1, FaceName = "MS Shell Dlg" }, 96, 292, 274, 27,
        ["Графическое приложение", "Консольное приложение", "Graphical application", "Command-line application",
        "Окно, уведомления и значок в трее. Использует WebView2.", "Работа через терминал. Добавляет папку программы в PATH.",
        "Window, notifications and tray icon. Uses WebView2.", "Terminal commands. Adds the installation folder to PATH."], 4);
    static object[] MeasureLayout(IntPtr tree, IntPtr description, string[] strings)
    {
        IntPtr font = Native.Send(tree, 0x31, IntPtr.Zero, IntPtr.Zero); // WM_GETFONT
        Native.GetObject(font, Marshal.SizeOf<Native.LogFont>(), out var logFont);
        var rect = Native.Client(description);
        return MeasureStrings(logFont, Native.GetDpiForWindow(tree), Native.Client(tree).Right, rect.Right, rect.Bottom, strings, 2);
    }
    static object[] MeasureStrings(Native.LogFont font, uint actualDpi, int treeWidth, int descriptionWidth, int descriptionHeight, string[] strings, int labelCount)
    {
        var results = new List<object>();
        IntPtr dc = Native.CreateCompatibleDC(IntPtr.Zero);
        try
        {
            foreach (int dpi in new[] { 96, 144, 192 })
            {
                var scaled = font;
                scaled.Height = (int)Math.Round(font.Height * dpi / (double)actualDpi);
                IntPtr handle = Native.CreateFontIndirect(ref scaled);
                IntPtr old = Native.SelectObject(dc, handle);
                try
                {
                    int availableTree = (int)Math.Round(treeWidth * dpi / (double)actualDpi);
                    int availableDescription = (int)Math.Round(descriptionWidth * dpi / (double)actualDpi);
                    var measurements = strings.Select((text, index) =>
                    {
                        Native.GetTextExtentPoint32(dc, text, text.Length, out var size);
                        var wrapped = new Native.Rect { Right = availableDescription };
                        Native.DrawText(dc, text, text.Length, ref wrapped, 0x400 | 0x10 | 0x800); // CALCRECT|WORDBREAK|NOPREFIX
                        int availableDescriptionHeight = (int)Math.Round(descriptionHeight * dpi / (double)actualDpi);
                        bool fits = index < labelCount ? size.X <= availableTree : wrapped.Bottom <= availableDescriptionHeight;
                        if (!fits) throw new InvalidOperationException($"GDI model at {dpi} DPI does not fit: {text}");
                        return new { text, role = index < labelCount ? "component-label" : "description", width = size.X,
                            wrappedHeight = wrapped.Bottom, availableTree, availableDescription, availableDescriptionHeight, fits };
                    }).ToArray();
                    results.Add(new { dpi, scalePercent = dpi * 100 / 96, evidence = "GDI measurement only; not a real window at this DPI", font = scaled, measurements });
                }
                finally { Native.SelectObject(dc, old); Native.DeleteObject(handle); }
            }
        }
        finally { Native.DeleteDC(dc); }
        return results.ToArray();
    }
}

sealed class RemoteMemory : IDisposable
{
    readonly IntPtr process;
    readonly IntPtr buffer;
    readonly bool widePointers;
    public RemoteMemory(int id)
    {
        process = Native.OpenProcess(0x38 | 0x400, false, (uint)id);
        if (process == IntPtr.Zero) throw new InvalidOperationException("Could not open the owned setup for tree text inspection.");
        if (!Native.IsWow64Process(process, out bool wow64)) throw new InvalidOperationException("Could not determine owned setup architecture.");
        widePointers = Environment.Is64BitOperatingSystem && !wow64;
        buffer = Native.VirtualAllocEx(process, IntPtr.Zero, 4096, 0x3000, 4);
        if (buffer == IntPtr.Zero) throw new InvalidOperationException("Could not allocate owned setup inspection buffer.");
    }
    void Write(byte[] data) { if (!Native.WriteProcessMemory(process, buffer, data, data.Length, out _)) throw new InvalidOperationException("Tree inspection write failed."); }
    byte[] Read(int length) { var data = new byte[length]; if (!Native.ReadProcessMemory(process, buffer, data, length, out _)) throw new InvalidOperationException("Tree inspection read failed."); return data; }
    public (string, int) Item(IntPtr tree, IntPtr item)
    {
        var data = new byte[4096];
        BitConverter.GetBytes(0x19).CopyTo(data, 0); // TEXT|STATE|HANDLE
        if (widePointers)
        {
            BitConverter.GetBytes(item.ToInt64()).CopyTo(data, 8);
            BitConverter.GetBytes(0xF000).CopyTo(data, 20);
            BitConverter.GetBytes(buffer.ToInt64() + 128).CopyTo(data, 24);
            BitConverter.GetBytes(1024).CopyTo(data, 32);
        }
        else
        {
            BitConverter.GetBytes((int)item.ToInt64()).CopyTo(data, 4);
            BitConverter.GetBytes(0xF000).CopyTo(data, 12);
            BitConverter.GetBytes((int)buffer.ToInt64() + 128).CopyTo(data, 16);
            BitConverter.GetBytes(1024).CopyTo(data, 20);
        }
        Write(data);
        if (Native.Send(tree, 0x113E, IntPtr.Zero, buffer) == IntPtr.Zero) throw new InvalidOperationException("TVM_GETITEMW failed.");
        data = Read(4096);
        return (Encoding.Unicode.GetString(data, 128, 2048).Split('\0')[0], (BitConverter.ToInt32(data, widePointers ? 16 : 8) >> 12) & 0xF);
    }
    public Native.Rect ItemRect(IntPtr tree, IntPtr item)
    {
        var data = new byte[16];
        (widePointers ? BitConverter.GetBytes(item.ToInt64()) : BitConverter.GetBytes((int)item.ToInt64())).CopyTo(data, 0);
        Write(data);
        if (Native.Send(tree, 0x1104, new IntPtr(1), buffer) == IntPtr.Zero) throw new InvalidOperationException("TVM_GETITEMRECT failed.");
        data = Read(16);
        return new() { Left = BitConverter.ToInt32(data, 0), Top = BitConverter.ToInt32(data, 4), Right = BitConverter.ToInt32(data, 8), Bottom = BitConverter.ToInt32(data, 12) };
    }
    public void Dispose() { if (buffer != IntPtr.Zero) Native.VirtualFreeEx(process, buffer, 0, 0x8000); if (process != IntPtr.Zero) Native.CloseHandle(process); }
}

static class Native
{
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct ScrollBarInfo { public uint Size; public Rect Bounds; public int LineButton, ThumbTop, ThumbBottom, Reserved; [MarshalAs(UnmanagedType.ByValArray, SizeConst = 6)] public uint[] State; }
    [StructLayout(LayoutKind.Sequential, CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public struct LogFont
    {
        public int Height, Width, Escapement, Orientation, Weight;
        public byte Italic, Underline, StrikeOut, CharSet, OutPrecision, ClipPrecision, Quality, PitchAndFamily;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string FaceName;
    }
    [StructLayout(LayoutKind.Sequential)] struct BitmapInfo { public uint Size; public int Width, Height; public ushort Planes, BitCount; public uint Compression, ImageSize; public int XPels, YPels; public uint Used, Important; }
    public delegate bool Enumerate(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool EnumWindows(Enumerate callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, Enumerate callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr window);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr parent, int id);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll", EntryPoint = "SendMessageW")] public static extern IntPtr Send(IntPtr window, int message, IntPtr word, IntPtr parameter);
    [DllImport("user32.dll", EntryPoint = "SendMessageW", CharSet = CharSet.Unicode)] public static extern IntPtr SendText(IntPtr window, int message, IntPtr word, StringBuilder parameter);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, int message, IntPtr word, IntPtr parameter);
    [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr window, int index);
    [DllImport("user32.dll")] public static extern bool GetScrollBarInfo(IntPtr window, int id, ref ScrollBarInfo info);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll", EntryPoint = "mouse_event")] public static extern void MouseEvent(uint flags, uint x, uint y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll")] public static extern bool IsWow64Process(IntPtr process, out bool wow64);
    [DllImport("kernel32.dll")] public static extern IntPtr VirtualAllocEx(IntPtr process, IntPtr address, uint size, uint allocation, uint protection);
    [DllImport("kernel32.dll")] public static extern bool VirtualFreeEx(IntPtr process, IntPtr address, uint size, uint type);
    [DllImport("kernel32.dll")] public static extern bool ReadProcessMemory(IntPtr process, IntPtr address, byte[] buffer, int size, out IntPtr read);
    [DllImport("kernel32.dll")] public static extern bool WriteProcessMemory(IntPtr process, IntPtr address, byte[] buffer, int size, out IntPtr written);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr handle);
    [DllImport("gdi32.dll", CharSet = CharSet.Unicode)] public static extern int GetObject(IntPtr font, int size, out LogFont result);
    [DllImport("gdi32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr CreateFontIndirect(ref LogFont font);
    [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr value);
    [DllImport("gdi32.dll")] public static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr value);
    [DllImport("gdi32.dll", CharSet = CharSet.Unicode)] public static extern bool GetTextExtentPoint32(IntPtr dc, string text, int length, out Point size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int DrawText(IntPtr dc, string text, int length, ref Rect rect, int format);
    [DllImport("user32.dll")] static extern IntPtr GetWindowDC(IntPtr window);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr window, IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int width, int height);
    [DllImport("user32.dll")] static extern bool RedrawWindow(IntPtr window, IntPtr rect, IntPtr region, uint flags);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr target, int x, int y, int width, int height, IntPtr source, int sourceX, int sourceY, uint operation);
    [DllImport("dwmapi.dll")] static extern int DwmFlush();
    [DllImport("gdi32.dll")] static extern int GetDIBits(IntPtr dc, IntPtr bitmap, uint start, uint lines, byte[] pixels, ref BitmapInfo info, uint usage);
    public static string Text(IntPtr window) { var value = new StringBuilder(8192); SendText(window, 0xD, new IntPtr(value.Capacity), value); return value.ToString(); }
    public static string Class(IntPtr window) { var value = new StringBuilder(256); GetClassName(window, value, value.Capacity); return value.ToString(); }
    public static Rect Bounds(IntPtr window) { GetWindowRect(window, out var rect); return rect; }
    public static Rect Client(IntPtr window) { GetClientRect(window, out var rect); return rect; }
    public static void SaveWindowBmp(IntPtr window, string path)
    {
        // Native description callbacks update text before their paint finishes.
        Thread.Sleep(200);
        RedrawWindow(window, IntPtr.Zero, IntPtr.Zero, 0x180); // ALLCHILDREN|UPDATENOW
        DwmFlush();
        var rect = Bounds(window);
        int width = rect.Right - rect.Left, height = rect.Bottom - rect.Top;
        IntPtr windowDc = GetWindowDC(window), dc = CreateCompatibleDC(windowDc), bitmap = CreateCompatibleBitmap(windowDc, width, height), old = SelectObject(dc, bitmap);
        try
        {
            if (!BitBlt(dc, 0, 0, width, height, windowDc, 0, 0, 0xCC0020))
                throw new InvalidOperationException("Could not capture visible pixels of the owned setup window.");
            SelectObject(dc, old);
            var info = new BitmapInfo { Size = 40, Width = width, Height = height, Planes = 1, BitCount = 32, ImageSize = (uint)(width * height * 4) };
            var pixels = new byte[info.ImageSize];
            if (GetDIBits(dc, bitmap, 0, (uint)height, pixels, ref info, 0) == 0) throw new InvalidOperationException("Could not read owned setup screenshot.");
            using var writer = new BinaryWriter(File.Create(path));
            writer.Write((ushort)0x4D42); writer.Write(54 + pixels.Length); writer.Write(0); writer.Write(54);
            writer.Write(40); writer.Write(width); writer.Write(height); writer.Write((ushort)1); writer.Write((ushort)32);
            writer.Write(0); writer.Write(pixels.Length); writer.Write(0); writer.Write(0); writer.Write(0); writer.Write(0); writer.Write(pixels);
        }
        finally { SelectObject(dc, old); DeleteObject(bitmap); DeleteDC(dc); ReleaseDC(window, windowDc); }
    }
}
