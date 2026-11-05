// M12 spike, not product code. Steps and what to report: README.md.
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text.RegularExpressions;
using LibreHardwareMonitor.Hardware;
using PawnIoDriver = LibreHardwareMonitor.PawnIo.PawnIo;

const int Duty = 60;
const int HoldSeconds = 10;
const int MovedRpm = 100;
const int ServiceWaitSeconds = 20;
const string Service = "PawnIO";

var command = args.FirstOrDefault();
switch (command)
{
    case "selftest":
        return SelfTest();
    case "list" or "driver" when args.Length == 1:
    case "fan" when args.Length == 2:
        break;
    default:
        Console.Error.WriteLine("usage: m12-spike selftest | list | fan <control id from list> | driver");
        return 2;
}

if (!new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator))
    return Fail("run this from an elevated terminal");
if (!PawnIoDriver.IsInstalled)
    return Fail("PawnIO is not installed (see README.md)");

// Two programs driving the same fan registers would invalidate the result and could undo the hand-back.
var rivals = new[] { "FanControl", "LibreHardwareMonitor", "OpenRGB" }
    .Where(name => Process.GetProcessesByName(name).Length > 0).ToList();
if (rivals.Count > 0)
    return Fail($"close these first: {string.Join(", ", rivals)}");
if (Process.GetProcessesByName("HWiNFO64").Length > 0)
    Console.WriteLine("note: HWiNFO64 is running. It may hold PawnIO open, so close it before `driver`.");

Console.WriteLine($"PawnIO {PawnIoDriver.Version}");
return command switch
{
    "list" => List(),
    "fan" => Fan(args[1]),
    _ => Driver(),
};

static int Fail(string message)
{
    Console.Error.WriteLine($"error: {message}");
    return 1;
}

static Computer Open()
{
    var computer = new Computer { IsMotherboardEnabled = true };
    computer.Open();
    Refresh(computer);
    Thread.Sleep(1000); // the first read of a chip can be empty
    Refresh(computer);
    return computer;
}

static IEnumerable<IHardware> Walk(IHardware h) => h.SubHardware.SelectMany(Walk).Prepend(h);

static IEnumerable<IHardware> AllHardware(Computer c) => c.Hardware.SelectMany(Walk);

static IEnumerable<ISensor> SensorsOf(Computer c, SensorType type) =>
    AllHardware(c).SelectMany(h => h.Sensors).Where(s => s.SensorType == type);

static void Refresh(Computer c)
{
    foreach (var h in AllHardware(c))
        h.Update();
}

static string Show(float? value, string unit) => value is { } v ? $"{v:F0}{unit}" : "no reading";

static void WriteReport(Computer c, string name)
{
    Directory.CreateDirectory("out");
    var path = Path.Combine("out", $"{name}.txt");
    File.WriteAllText(path, c.GetReport());
    Console.WriteLine($"Registers: {Path.GetFullPath(path)}");
}

static int List()
{
    var computer = Open();
    try
    {
        var chips = AllHardware(computer).Where(h => h.HardwareType == HardwareType.SuperIO).ToList();
        foreach (var chip in chips)
        {
            Console.WriteLine($"\nSuper I/O chip: {chip.Name}  ({chip.Identifier})");
            foreach (var s in chip.Sensors.Where(s => s.SensorType == SensorType.Fan))
                Console.WriteLine($"  fan      {s.Identifier,-34} {s.Name,-22} {Show(s.Value, " RPM")}");
            foreach (var s in chip.Sensors.Where(s => s.SensorType == SensorType.Control))
                Console.WriteLine($"  control  {s.Identifier,-34} {s.Name,-22} {Show(s.Value, "%"),-10} mode={s.Control?.ControlMode.ToString() ?? "not controllable"}");
        }
        WriteReport(computer, "list");
        Console.WriteLine("\nmode comes from LHM, not the chip: Undefined only means this program has not touched it.");
        Console.WriteLine("The chip's real mode is in the register dump above.");
        return chips.Count > 0 ? 0 : Fail("no Super I/O chip found, so PawnIO may have refused the module or the board is unsupported");
    }
    finally
    {
        computer.Close();
    }
}

static int Fan(string id)
{
    using var stop = new CancellationTokenSource();
    PosixSignalRegistration[] signals = new[] { PosixSignal.SIGINT, PosixSignal.SIGQUIT, PosixSignal.SIGTERM, PosixSignal.SIGHUP }
        .Select(signal => PosixSignalRegistration.Create(signal, context =>
        {
            context.Cancel = true; // keep running so the hand-back below happens
            stop.Cancel();
        }))
        .ToArray();

    var computer = Open();
    try
    {
        var sensor = SensorsOf(computer, SensorType.Control).FirstOrDefault(s => s.Identifier.ToString() == id);
        if (sensor?.Control is not { } control)
            return Fail($"{id} is not a control. Run `list` and copy a control id");
        // shortcut: a name check only catches labelled headers, the real pump rule comes with FanKind in M12.
        if (IsCoolerHeader(sensor.Name))
            return Fail($"refusing {sensor.Name}: this spike only drives a case fan");

        var before = Fans(computer);
        Print("before", computer, sensor);
        WriteReport(computer, "fan-before");

        Dictionary<string, float?> held;
        try
        {
            control.SetSoftware(Duty);
            Console.WriteLine($"\nset {sensor.Name} to {Duty}% for {HoldSeconds} s (Ctrl-C hands back early)");
            for (var waited = 0; waited < HoldSeconds && !stop.IsCancellationRequested; waited += 2)
            {
                stop.Token.WaitHandle.WaitOne(TimeSpan.FromSeconds(2));
                Refresh(computer);
                Print($"t+{waited + 2,2}s", computer, sensor);
            }
            held = Fans(computer);
            WriteReport(computer, "fan-held");
        }
        finally
        {
            // A hard kill or power loss skips this: the fan keeps this duty until the next reboot.
            control.SetDefault();
            Console.WriteLine($"\nhanded back: mode={control.ControlMode}");
        }

        var moved = before.Keys.Where(fan => Moved(before[fan], held.GetValueOrDefault(fan))).ToList();
        foreach (var fan in moved)
            Console.WriteLine($"RPM changed while held: {fan} {Show(before[fan], "")} -> {Show(held[fan], " RPM")} (a BIOS curve can do this too, so it only counts on the fan you set)");
        if (moved.Count == 0)
            Console.WriteLine($"NO fan moved by {MovedRpm} RPM or more. Inconclusive unless it already ran near {Duty}%.");

        // Long enough for the BIOS curve to take the fan back and for the RPM to settle.
        for (var i = 0; i < 5; i++)
        {
            Thread.Sleep(1000);
            Refresh(computer);
        }
        Print("after", computer, sensor);
        WriteReport(computer, "fan-after");
        return 0;
    }
    finally
    {
        computer.Close(); // also restores any control still in software mode
        GC.KeepAlive(signals);
    }
}

static Dictionary<string, float?> Fans(Computer c) =>
    SensorsOf(c, SensorType.Fan).ToDictionary(s => s.Identifier.ToString(), s => s.Value);

static void Print(string label, Computer c, ISensor control)
{
    var fans = SensorsOf(c, SensorType.Fan).Select(s => $"{s.Name}={Show(s.Value, "")}");
    Console.WriteLine($"{label,-7} control={Show(control.Value, "%")}  {string.Join("  ", fans)}");
}

static bool Moved(float? before, float? after) =>
    before is { } b && after is { } a && Math.Abs(a - b) >= MovedRpm;

static bool IsCoolerHeader(string name) =>
    name.Contains("cpu", StringComparison.OrdinalIgnoreCase) || name.Contains("pump", StringComparison.OrdinalIgnoreCase);

// No Computer is open in this path until the driver is back, so none of our handles can block the unload.
static int Driver()
{
    var initial = QueryState();
    if (initial != "4")
        return Fail($"{Service} is not running (state {initial}), so there is nothing to unload");

    Console.WriteLine($"\n> sc stop {Service}");
    var stopExit = Sc("stop");
    if (!WaitFor("1"))
    {
        Console.WriteLine($"RESULT unload: NO. sc stop exit {stopExit}, state {QueryState()}. See README.md for the next step.");
        return 1;
    }
    Console.WriteLine("RESULT unload: YES, stopped with no reboot.");

    Console.WriteLine($"\n> sc start {Service}");
    var startExit = Sc("start");
    if (!WaitFor("4"))
    {
        Console.WriteLine($"RESULT restart: NO. sc start exit {startExit}, state {QueryState()}.");
        return 1;
    }

    // Running is not enough: the library has to load its module and read the chip again.
    var computer = Open();
    try
    {
        var chip = AllHardware(computer).FirstOrDefault(h => h.HardwareType == HardwareType.SuperIO);
        var spinning = SensorsOf(computer, SensorType.Fan).Count(s => s.Value > 0);
        var clean = chip is not null && spinning > 0;
        Console.WriteLine(clean
            ? $"RESULT restart: YES. {chip!.Name} reads {spinning} spinning fan(s)."
            : $"RESULT restart: NO. Service is running, but chip={chip?.Name ?? "none"} and {spinning} fan(s) read.");
        return clean ? 0 : 1;
    }
    finally
    {
        computer.Close();
    }
}

static bool WaitFor(string state)
{
    for (var i = 0; i < ServiceWaitSeconds; i++)
    {
        if (QueryState() == state)
            return true;
        Thread.Sleep(1000);
    }
    return QueryState() == state;
}

static string? QueryState() => ParseState(RunSc("query").Output);

static int Sc(string verb)
{
    var (exit, output) = RunSc(verb);
    Console.WriteLine(output.Trim());
    return exit;
}

static (int Exit, string Output) RunSc(string verb)
{
    var info = new ProcessStartInfo("sc.exe", [verb, Service]) { RedirectStandardOutput = true, UseShellExecute = false };
    using var process = Process.Start(info)!;
    var output = process.StandardOutput.ReadToEnd();
    process.WaitForExit();
    return (process.ExitCode, output);
}

// shortcut: expects sc.exe's English STATE label, the bridge must use the service API instead.
static string? ParseState(string scQuery) =>
    Regex.Match(scQuery, @"^\s*STATE\s*:\s*(\d)", RegexOptions.Multiline) is { Success: true } m ? m.Groups[1].Value : null;

static int SelfTest()
{
    const string running = "SERVICE_NAME: PawnIO \r\n        TYPE               : 1  KERNEL_DRIVER  \r\n        STATE              : 4  RUNNING \r\n                                (STOPPABLE, NOT_PAUSABLE, IGNORES_SHUTDOWN)\r\n        WIN32_EXIT_CODE    : 0  (0x0)\r\n";
    (bool Ok, string Name)[] checks =
    [
        (ParseState(running) == "4", "running state parsed, TYPE ignored"),
        (ParseState(running.Replace("4  RUNNING", "1  STOPPED")) == "1", "stopped state parsed"),
        (ParseState("[SC] EnumQueryServicesStatus:OpenService FAILED 1060:") is null, "failed query gives no state"),
        (IsCoolerHeader("CPU Fan") && IsCoolerHeader("AIO Pump"), "CPU and pump headers refused"),
        (!IsCoolerHeader("Chassis Fan #1") && !IsCoolerHeader("Fan #2"), "case fans allowed"),
        (Moved(1000f, 1200f) && Moved(1200f, 1000f), "RPM change counts both ways"),
        (!Moved(1000f, 1050f) && !Moved(null, 1200f) && !Moved(1000f, null), "small change or no reading is not movement"),
    ];
    foreach (var (ok, name) in checks)
        Console.WriteLine($"{(ok ? "ok  " : "FAIL")} {name}");
    return checks.All(c => c.Ok) ? 0 : 1;
}
