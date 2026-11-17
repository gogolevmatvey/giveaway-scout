using System.Diagnostics;
using System.Runtime.InteropServices;
using FlaUI.Core;
using FlaUI.Core.AutomationElements;
using FlaUI.Core.Definitions;
using FlaUI.UIA3;

// Bootstrap deliberately does not launch, close or click the user's application.
// --self-check proves that UIA3 initializes, not that Giveaway Scout passed E2E.
if (!OperatingSystem.IsWindows())
{
    Console.Error.WriteLine("FlaUI requires Windows; run with Windows dotnet, not WSL dotnet.");
    return 2;
}

try
{
    using var automation = new UIA3Automation();
    if (args is ["--self-check"])
    {
        _ = automation.GetDesktop();
        Console.WriteLine("PASS: FlaUI.UIA3 initialized. No application acceptance test was run.");
        return 0;
    }

    var validArguments = args is ["--inspect-pid" or "--test-pid", _]
        or ["--folder-dialog", _, "cancel" or "select", _]
        or ["--watch-log-folder", _, "default" or "custom", _];
    if (!validArguments
        || !int.TryParse(args[1], out var pid) || pid <= 0)
    {
        Console.Error.WriteLine("Usage: --self-check | --inspect-pid <PID> | --test-pid <TEST PID> | --folder-dialog <TEST PID> cancel|select <owned root> | --watch-log-folder <TEST PID> default|custom <owned root>");
        return 2;
    }

    using var process = Process.GetProcessById(pid);
    if (!string.Equals(process.ProcessName, "giveaway-scout", StringComparison.OrdinalIgnoreCase))
        throw new InvalidOperationException("Refusing to inspect a process other than giveaway-scout.exe.");

    using var app = Application.Attach(pid);
    var windows = app.GetAllTopLevelWindows(automation);
    if (windows.Length == 0)
        throw new InvalidOperationException("No accessible window. Open Giveaway Scout from its tray first.");

    if (args[0] != "--inspect-pid")
    {
        var window = UniqueOwnedVisibleWindow(automation, windows, pid,
            name => name == "Giveaway Scout [TEST]", "isolated TEST main window")
            ?? throw new InvalidOperationException("Refusing actions: selected process is not the isolated TEST window.");
        if (args[0] is "--folder-dialog" or "--watch-log-folder")
        {
            var root = OwnedRoot(args[3]);
            var custom = Path.Combine(root, "custom-logs");
            if (args[0] == "--watch-log-folder")
            {
                WatchLogFolder(Path.Combine(root, args[2] == "custom" ? "custom-logs" : "logs"));
                return 0;
            }
            // An IFileDialog owned by the main HWND is not necessarily returned
            // by Application.GetAllTopLevelWindows. Search the desktop tree, but
            // require this verified TEST PID before considering any control.
            var observed = new HashSet<string>();
            Window? dialog = null;
            try
            {
                dialog = WaitFor(() =>
                {
                    var ownedWindows = automation.GetDesktop().FindAllDescendants(cf =>
                        cf.ByProcessId(pid).And(cf.ByControlType(ControlType.Window)));
                    foreach (var ownedWindow in ownedWindows)
                        observed.Add($"{ownedWindow.Properties.Name.ValueOrDefault} [{ownedWindow.Properties.ClassName.ValueOrDefault}] HWND={NativeHandle(ownedWindow)}");
                    return UniqueOwnedVisibleWindow(automation, ownedWindows, pid,
                        IsFolderDialogName, "owned native folder dialog");
                }, "owned native folder dialog");

                if (args[2] == "select")
                {
                    RejectLink(custom);
                    if (!Directory.Exists(custom)) throw new InvalidOperationException("Test custom folder is missing.");
                    SelectOwnedFolder(dialog, custom, pid);
                }
                else
                {
                    var dialogHandle = NativeHandle(dialog);
                    WaitFor(() => dialog.FindFirstDescendant(cf => cf.ByControlType(ControlType.Button)
                        .And(cf.ByAutomationId("2"))), "native Cancel button").AsButton().Invoke();
                    WaitFor(() => IsOwnedVisibleHandle(dialogHandle, pid) ? null : dialog, "owned folder dialog closed");
                }
            }
            catch
            {
                // These are only native-window metadata from the owned TEST
                // process; no other application's UI contents are inspected.
                foreach (var description in observed.Order())
                    Console.Error.WriteLine($"TEST process window: {description}");
                if (dialog is not null) CancelOwnedFolderDialog(dialog, pid);
                throw;
            }
            Console.WriteLine($"PASS: native folder dialog {args[2]} completed for the isolated TEST process.");
            return 0;
        }
        AutomationElement FindButton(string name)
        {
            var timer = Stopwatch.StartNew();
            while (timer.Elapsed < TimeSpan.FromSeconds(7))
            {
                var element = window.FindFirstDescendant(cf => cf.ByControlType(ControlType.Button).And(cf.ByName(name)));
                if (element is not null) return element;
                Thread.Sleep(100);
            }
            throw new InvalidOperationException($"TEST button not accessible: {name}");
        }
        FindButton("Settings").AsButton().Invoke();
        FindButton("Close settings").AsButton().Invoke();
        Console.WriteLine("PASS: FlaUI invoked Settings and Close settings in the isolated TEST window.");
        return 0;
    }

    Console.WriteLine($"PASS: UIA3 can read {windows.Length} window(s) of the explicitly selected process.");
    // Avoid dumping user history, titles, URLs or control contents.
    Console.WriteLine("Read-only inspection only; no controls were invoked and no settings were changed.");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine($"FAIL: {error}");
    return 1;
}

static T WaitFor<T>(Func<T?> action, string label) where T : class
{
    var timer = Stopwatch.StartNew();
    while (timer.Elapsed < TimeSpan.FromSeconds(12))
    {
        var result = action();
        if (result is not null) return result;
        Thread.Sleep(100);
    }
    throw new InvalidOperationException($"Timed out: {label}");
}

static nint NativeHandle(AutomationElement element) => (nint)element.Properties.NativeWindowHandle.ValueOrDefault;

static bool IsOwnedVisibleWindow(AutomationElement window, int pid)
    => IsOwnedVisibleHandle(NativeHandle(window), pid);

static bool IsOwnedVisibleHandle(nint handle, int pid)
{
    return handle != 0 && OwnedWindowsApi.IsWindowVisible(handle)
        && OwnedWindowsApi.GetWindowThreadProcessId(handle, out var ownerPid) != 0
        && ownerPid == (uint)pid;
}

static Window? UniqueOwnedVisibleWindow(UIA3Automation automation,
    IEnumerable<AutomationElement> candidates, int pid, Func<string, bool> matches, string label)
{
    // UIA can expose multiple elements/providers for the same native window.
    // Compare HWNDs, require visibility and verify ownership through Win32;
    // never select an arbitrary distinct window if the match is ambiguous.
    var handles = candidates.Where(candidate => matches(candidate.Properties.Name.ValueOrDefault ?? "")
            && IsOwnedVisibleWindow(candidate, pid))
        .Select(NativeHandle).Distinct().ToArray();
    if (handles.Length > 1)
        throw new InvalidOperationException($"Ambiguous {label}: {handles.Length} visible owned HWNDs ({string.Join(", ", handles)}).");
    return handles.Length == 0 ? null : automation.FromHandle(handles[0]).AsWindow();
}

static bool IsFolderDialogName(string name) => name is "Choose log folder" or "Выберите папку журналов";

static void CancelOwnedFolderDialog(Window dialog, int pid)
{
    try
    {
        if (!IsOwnedVisibleWindow(dialog, pid) || !IsFolderDialogName(dialog.Properties.Name.ValueOrDefault ?? ""))
            return;
        dialog.FindFirstDescendant(cf => cf.ByControlType(ControlType.Button)
            .And(cf.ByAutomationId("2")))?.AsButton().Invoke();
    }
    catch (Exception cleanupError)
    {
        Console.Error.WriteLine($"TEST folder cleanup unavailable: {cleanupError.GetType().Name}");
    }
}

static void SelectOwnedFolder(Window dialog, string expected, int pid)
{
    var dialogHandle = NativeHandle(dialog);
    try
    {
        // Use the common dialog's filename/folder-name control directly. Global
        // Ctrl+L and keyboard input can target a different HWND after a focus
        // change, and breadcrumb names differ between Windows builds/languages.
        var input = WaitFor(() => FolderNameInput(dialog), "native folder-name input");
        input.Patterns.Value.Pattern.SetValue(expected);
        WaitFor(() => string.Equals(input.Patterns.Value.Pattern.Value.ValueOrDefault, expected,
            StringComparison.OrdinalIgnoreCase) ? input : null, "owned folder-name value");
        SelectFolderButton(dialog).AsButton().Invoke();

        var timer = Stopwatch.StartNew();
        var selectedCurrentFolder = false;
        while (timer.Elapsed < TimeSpan.FromSeconds(12))
        {
            if (!IsOwnedVisibleHandle(dialogHandle, pid)) return;
            // Some common-dialog versions navigate to a typed directory first.
            // Select the current directory only after its complete owned path is
            // observable in the address control, never from a basename match.
            if (!selectedCurrentFolder && ShowsFolderAddress(dialog, expected))
            {
                var currentInput = FolderNameInput(dialog);
                if (currentInput is not null)
                {
                    currentInput.Patterns.Value.Pattern.SetValue("");
                    SelectFolderButton(dialog).AsButton().Invoke();
                    selectedCurrentFolder = true;
                }
            }
            Thread.Sleep(100);
        }
        throw new InvalidOperationException("Owned native folder dialog did not finish selecting the exact test folder.");
    }
    catch
    {
        DumpFolderControls(dialog);
        // A failed fixture must not leave its own modal picker blocking later
        // assertions. Cancellation is confined to this already verified HWND.
        CancelOwnedFolderDialog(dialog, pid);
        throw;
    }
}

static AutomationElement? FolderNameInput(Window dialog)
{
    // edt1 (1152) and cmb13 (1148) are the common-dialog controls. On some
    // Windows versions cmb13 exposes its editable child instead of edt1.
    var input = dialog.FindFirstDescendant(cf => cf.ByControlType(ControlType.Edit)
        .And(cf.ByAutomationId("1152").Or(cf.ByAutomationId("1148"))));
    if (input?.Patterns.Value.IsSupported == true) return input;
    var combo = dialog.FindFirstDescendant(cf => cf.ByAutomationId("1148"));
    return combo?.FindAllDescendants(cf => cf.ByControlType(ControlType.Edit))
        .FirstOrDefault(element => element.Patterns.Value.IsSupported);
}

static AutomationElement SelectFolderButton(Window dialog) =>
    WaitFor(() => dialog.FindFirstDescendant(cf => cf.ByControlType(ControlType.Button)
        .And(cf.ByAutomationId("1"))), "native Select Folder button");

static bool ShowsFolderAddress(Window dialog, string expected)
{
    foreach (var control in dialog.FindAllDescendants(cf => cf.ByControlType(ControlType.ToolBar)
        .Or(cf.ByAutomationId("41477"))))
    {
        if (control.Properties.Name.ValueOrDefault?.Contains(expected, StringComparison.OrdinalIgnoreCase) == true)
            return true;
        if (control.Patterns.Value.IsSupported && string.Equals(
            control.Patterns.Value.Pattern.Value.ValueOrDefault, expected, StringComparison.OrdinalIgnoreCase))
            return true;
    }
    return false;
}

static void DumpFolderControls(Window dialog)
{
    // Only metadata of navigation controls under the verified TEST-owned dialog;
    // no desktop-wide contents, directory listing, or other app is dumped.
    try
    {
        foreach (var control in dialog.FindAllDescendants(cf => cf.ByControlType(ControlType.Edit)
            .Or(cf.ByControlType(ControlType.ComboBox)).Or(cf.ByControlType(ControlType.ToolBar))
            .Or(cf.ByControlType(ControlType.Button))))
        {
            Console.Error.WriteLine($"TEST folder control: {control.Properties.ControlType.ValueOrDefault}" +
                $" id={control.Properties.AutomationId.ValueOrDefault} class={control.Properties.ClassName.ValueOrDefault}" +
                $" name={control.Properties.Name.ValueOrDefault}");
        }
    }
    catch (Exception diagnosticError)
    {
        Console.Error.WriteLine($"TEST folder metadata unavailable: {diagnosticError.GetType().Name}");
    }
}

static void RejectLink(string path)
{
    if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
        throw new InvalidOperationException("Test-owned paths must not be links or junctions.");
}

static string OwnedRoot(string path)
{
    var root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));
    var temp = Path.TrimEndingDirectorySeparator(Path.GetFullPath(Path.GetTempPath()));
    var name = Path.GetFileName(root);
    const string prefix = "giveaway-scout-e2e-";
    if (!string.Equals(Path.GetDirectoryName(root), temp, StringComparison.OrdinalIgnoreCase)
        || !name.StartsWith(prefix, StringComparison.Ordinal)
        || !Guid.TryParse(name[prefix.Length..], out _))
        throw new InvalidOperationException("Refusing actions outside a direct owned temp profile.");
    RejectLink(root);
    var marker = Path.Combine(root, ".giveaway-scout-ui-test");
    RejectLink(marker);
    if (File.ReadAllText(marker) != "Giveaway Scout isolated UI tests v1\n")
        throw new InvalidOperationException("Test profile ownership marker is missing.");
    return root;
}

static void WatchLogFolder(string expected)
{
    RejectLink(expected);
    var shellType = Type.GetTypeFromProgID("Shell.Application")
        ?? throw new InvalidOperationException("Explorer automation is unavailable.");
    dynamic shell = Activator.CreateInstance(shellType)!;
    try
    {
        var before = ShellFolders((object)shell).Select(item => item.Handle).ToHashSet();
        Console.WriteLine("READY: watching only the owned test log folder.");
        Console.Out.Flush();
        var timer = Stopwatch.StartNew();
        while (timer.Elapsed < TimeSpan.FromSeconds(12))
        {
            var found = ShellFolders((object)shell).FirstOrDefault(item => string.Equals(
                item.Path, expected, StringComparison.OrdinalIgnoreCase));
            if (found != default)
            {
                // Never close a pre-existing Explorer window. Only a new HWND
                // still showing this exact synthetic folder belongs to this test.
                if (!before.Contains(found.Handle)) CloseNewShellFolder((object)shell, found.Handle, expected);
                Console.WriteLine("PASS: Explorer opened the actual isolated log folder.");
                return;
            }
            Thread.Sleep(100);
        }
        throw new InvalidOperationException("Explorer did not show the expected active log folder.");
    }
    finally
    {
        Marshal.FinalReleaseComObject(shell);
    }
}

static List<(long Handle, string Path)> ShellFolders(dynamic shell)
{
    var result = new List<(long Handle, string Path)>();
    dynamic windows = shell.Windows();
    try
    {
        for (var i = 0; i < (int)windows.Count; i++)
        {
            dynamic? item = windows.Item(i);
            if (item is null) continue;
            try
            {
                string url = item.LocationURL;
                if (Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.IsFile)
                {
                    long handle = Convert.ToInt64(item.HWND);
                    result.Add((handle, Path.TrimEndingDirectorySeparator(uri.LocalPath)));
                }
            }
            catch (COMException) { /* An unrelated Explorer window may close while inspecting. */ }
            finally { Marshal.FinalReleaseComObject(item); }
        }
    }
    finally { Marshal.FinalReleaseComObject(windows); }
    return result;
}

static void CloseNewShellFolder(dynamic shell, long handle, string expected)
{
    dynamic windows = shell.Windows();
    try
    {
        for (var i = 0; i < (int)windows.Count; i++)
        {
            dynamic? item = windows.Item(i);
            if (item is null) continue;
            try
            {
                string url = item.LocationURL;
                long currentHandle = Convert.ToInt64(item.HWND);
                if (currentHandle == handle && Uri.TryCreate(url, UriKind.Absolute, out var uri)
                    && uri.IsFile && string.Equals(Path.TrimEndingDirectorySeparator(uri.LocalPath), expected,
                        StringComparison.OrdinalIgnoreCase))
                    item.Quit();
            }
            catch (COMException) { }
            finally { Marshal.FinalReleaseComObject(item); }
        }
    }
    finally { Marshal.FinalReleaseComObject(windows); }
}

static class OwnedWindowsApi
{
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool IsWindowVisible(nint handle);

    [DllImport("user32.dll")]
    internal static extern uint GetWindowThreadProcessId(nint handle, out uint processId);
}
