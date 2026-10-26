using System.Diagnostics;
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

    if (args.Length != 2 || args[0] is not ("--inspect-pid" or "--test-pid")
        || !int.TryParse(args[1], out var pid) || pid <= 0)
    {
        Console.Error.WriteLine("Usage: --self-check | --inspect-pid <PID> (read-only) | --test-pid <isolated test PID>");
        return 2;
    }

    using var process = Process.GetProcessById(pid);
    if (!string.Equals(process.ProcessName, "giveaway-scout", StringComparison.OrdinalIgnoreCase))
        throw new InvalidOperationException("Refusing to inspect a process other than giveaway-scout.exe.");

    using var app = Application.Attach(pid);
    var windows = app.GetAllTopLevelWindows(automation);
    if (windows.Length == 0)
        throw new InvalidOperationException("No accessible window. Open Giveaway Scout from its tray first.");

    if (args[0] == "--test-pid")
    {
        var window = windows.SingleOrDefault(w => w.Name == "Giveaway Scout [TEST]")
            ?? throw new InvalidOperationException("Refusing actions: selected process is not the isolated TEST window.");
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
    Console.Error.WriteLine($"FAIL: {error.GetType().Name}: {error.Message}");
    return 1;
}
