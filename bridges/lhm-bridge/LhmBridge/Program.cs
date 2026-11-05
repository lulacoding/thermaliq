using System.Runtime.InteropServices;
using LhmBridge;
using LibreHardwareMonitor.Hardware;
using PawnIoDriver = LibreHardwareMonitor.PawnIo.PawnIo;

const int ExitOk = 0;
const int ExitUsage = 2;
const int ExitNoPawnIo = 10;
const int ExitSelfCheck = 11;
const int ExitPipeTaken = 12;
const int ExitFailed = 13;
var watchdogTick = TimeSpan.FromMilliseconds(250);

if (args is not ["--pipe", var pipeName, "--data-dir", var dataDir])
{
    Console.Error.WriteLine("usage: lhm-bridge --pipe <name> --data-dir <dir>");
    return ExitUsage;
}

// Before anything can reach a fan: a changed library must never get as far as a takeover.
var problems = LhmInternals.Check(typeof(Computer).Assembly);
if (problems.Count > 0)
{
    Console.Error.WriteLine($"self-check failed: {string.Join("; ", problems)}");
    return ExitSelfCheck;
}
if (!PawnIoDriver.IsInstalled)
{
    Console.Error.WriteLine("PawnIO is not installed: motherboard fans are absent");
    return ExitNoPawnIo;
}

using var cancel = new CancellationTokenSource();
PosixSignalRegistration[] signals = new[] { PosixSignal.SIGINT, PosixSignal.SIGQUIT, PosixSignal.SIGTERM, PosixSignal.SIGHUP }
    .Select(signal => PosixSignalRegistration.Create(signal, context =>
    {
        context.Cancel = true; // keep running so the hand-back in PipeHost.Run happens
        cancel.Cancel();
    }))
    .ToArray();

System.IO.Pipes.NamedPipeServerStream server;
try
{
    server = PipeHost.Create(pipeName, PipeHost.SystemOnly());
}
catch (Exception e) when (e is IOException or UnauthorizedAccessException)
{
    Console.Error.WriteLine($"pipe {pipeName} is taken: {e.Message}");
    return ExitPipeTaken;
}

var computer = new Computer(new SeededSettings()) { IsMotherboardEnabled = true, IsCpuEnabled = true };
Bridge? bridge = null;
AppDomain.CurrentDomain.UnhandledException += (_, _) => bridge?.RestoreAll();
try
{
    computer.Open();
    var hardware = new LhmHardware(computer);
    bridge = new Bridge(hardware, new OriginalsStore(Path.Combine(dataDir, OriginalsStore.FileName)));
    bridge.Recover();
    foreach (var (fan, reason) in bridge.Degraded)
        Console.Error.WriteLine($"degraded {fan}: {reason}");
    bridge.Beat(); // a slow start isn't a silent client
    PipeHost.Run(server, bridge, watchdogTick, cancel.Token);
    return bridge.Degraded.Count == 0 ? ExitOk : ExitFailed;
}
catch (Exception e)
{
    Console.Error.WriteLine($"bridge failed: {e}");
    bridge?.RestoreAll();
    return ExitFailed;
}
finally
{
    computer.Close();
    GC.KeepAlive(signals);
}
