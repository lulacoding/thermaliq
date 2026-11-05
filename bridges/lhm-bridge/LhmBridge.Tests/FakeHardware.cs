using LhmBridge;

namespace LhmBridge.Tests;

/// Behaves like the pinned LHM on NCT6687D, including its first-write and hand-back order.
public sealed class FakeHardware : IFanHardware
{
    public readonly List<string> Log = [];
    public readonly Dictionary<string, Original> Registers = new();
    public readonly Dictionary<string, float> Stored = new();
    public readonly Dictionary<string, float> Actual = new();
    public readonly Dictionary<string, Original> Armed = new();
    public readonly HashSet<string> StuckManual = [];
    public bool DropWrites;
    public bool ReportUnreadable;
    public int DefaultsUntilClear;

    public FakeHardware(params Header[] headers)
    {
        Headers = headers;
        foreach (var h in headers)
        {
            Registers[h.Id] = new Original(0, 0x66);
            Stored[h.Id] = 100;
            Actual[h.Id] = 40;
        }
    }

    public IReadOnlyList<Header> Headers { get; }

    public Readings Read() => new([new SensorDto("t", "CPU", "cpu", 50)], [], []);

    public Original? ReadRegisters(Header h) => ReportUnreadable ? null : Registers[h.Id];

    public bool? IsManual(Header h) => ReportUnreadable ? null : Registers[h.Id].Mode != 0;

    public float StoredDuty(Header h) => Stored[h.Id];

    public float? ActualDuty(Header h) => Actual[h.Id];

    public void Write(Header h, byte duty)
    {
        Log.Add($"write {h.Id} {duty}");
        if (DropWrites) return;
        if (Stored[h.Id] == duty && Registers[h.Id].Mode != 0) return; // LHM ignores a repeated value
        Registers[h.Id] = Registers[h.Id] with { Mode = (byte)(1 << h.Index), Command = duty };
        Stored[h.Id] = duty;
        Actual[h.Id] = duty;
    }

    public void Arm(Header h, Original o)
    {
        Log.Add($"arm {h.Id} {o.Mode} {o.Command}");
        Armed[h.Id] = o;
    }

    public void Default(Header h)
    {
        Log.Add($"default {h.Id}");
        if (DefaultsUntilClear > 0)
        {
            DefaultsUntilClear--;
            return;
        }
        if (StuckManual.Contains(h.Id)) return;
        if (Armed.TryGetValue(h.Id, out var o)) Registers[h.Id] = o;
    }

    public static Header Case(int index = 5) =>
        new($"lhm_nct6687d_0_fan_{index}", $"System Fan #{index - 1}", HeaderKind.Fan, Chips.Nct6687D, index, true);

    public static Header Pump(int index = 1) =>
        new($"lhm_nct6687d_0_fan_{index}", "Pump Fan", HeaderKind.Pump, Chips.Nct6687D, index, true);

    public static Header OtherChip(int index = 2) =>
        new($"lhm_nct6798d_0_fan_{index}", "System Fan", HeaderKind.Fan, "NCT6798D", index, false);
}

public sealed class TempDir : IDisposable
{
    public readonly string Path = Directory.CreateTempSubdirectory("lhm-bridge-test").FullName;

    public string File => System.IO.Path.Combine(Path, OriginalsStore.FileName);

    public void Dispose() => Directory.Delete(Path, true);
}

public sealed class FakeClock
{
    public TimeSpan Now = TimeSpan.Zero;

    public void Sleep(TimeSpan by) => Now += by;
}
