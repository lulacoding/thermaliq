using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using LibreHardwareMonitor.Hardware;

namespace LhmBridge;

public static class Chips
{
    /// The only chip with measured restore behaviour (SPEC 5.1). Its save fields hold one bit per header.
    public const string Nct6687D = "NCT6687D";
}

public enum HeaderKind
{
    Fan,
    Pump,
}

public sealed record Header(string Id, string Label, HeaderKind Kind, string Chip, int Index, bool Controllable);

/// A header's manual flag (0 or `1 << index`) and PWM command byte.
public sealed record Original(byte Mode, byte Command);

public sealed record SensorDto(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("label")] string Label,
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("temp_celsius")] float TempCelsius);

public sealed record FanDto(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("label")] string Label,
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("rpm")] float? Rpm,
    [property: JsonPropertyName("duty_percent")] float? DutyPercent,
    [property: JsonPropertyName("controllable")] bool Controllable);

public sealed record MetricDto(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("value")] double Value);

public sealed record Readings(
    [property: JsonPropertyName("sensors")] IReadOnlyList<SensorDto> Sensors,
    [property: JsonPropertyName("fans")] IReadOnlyList<FanDto> Fans,
    [property: JsonPropertyName("metrics")] IReadOnlyList<MetricDto> Metrics);

/// What the bridge needs from LHM, so every rule can be tested without hardware.
public interface IFanHardware
{
    IReadOnlyList<Header> Headers { get; }

    /// The motherboard's name and its fan chip. The daemon saves it, so the same fan ids on
    /// another board are noticed.
    string Board { get; }

    Readings Read();

    /// The chip's registers right now. Null if they could not be read.
    Original? ReadRegisters(Header h);

    /// Null if the registers could not be read: unknown is never treated as clear.
    bool? IsManual(Header h);

    /// The value LHM writes for about 100 ms on a change to software mode, before the requested duty.
    float StoredDuty(Header h);

    /// The duty the chip reports now.
    float? ActualDuty(Header h);

    /// Public `SetSoftware`. LHM does nothing if the mode and value are unchanged.
    void Write(Header h, byte duty);

    /// Sets LHM's private save fields, ready for `Default`. Writes no register.
    void Arm(Header h, Original o);

    /// Public `SetDefault`. LHM does nothing if already in default mode.
    void Default(Header h);
}

public static partial class HeaderRules
{
    public static HeaderKind KindOf(string name) =>
        name.Contains("pump", StringComparison.OrdinalIgnoreCase) ? HeaderKind.Pump : HeaderKind.Fan;

    public static bool IsSupportedChip(string identifier) => SupportedChip().IsMatch(identifier);

    public static string FanId(string identifier) =>
        "lhm_" + identifier.TrimStart('/').Replace("lpc/", "").Replace('/', '_');

    /// One sensor only, because S2 reads every CPU sensor and the agent needs exactly one to bind.
    public static string? CpuSensor(IReadOnlyList<(string Id, HardwareType Type, string Name)> temperatures)
    {
        foreach (var (id, type, name) in temperatures)
            if (type == HardwareType.Cpu && name is "Core (Tctl/Tdie)" or "Core (Tdie)" or "CPU Package")
                return id;
        foreach (var (id, type, name) in temperatures)
            if (type == HardwareType.SuperIO && name == "CPU")
                return id;
        return null;
    }

    [GeneratedRegex(@"^/lpc/nct6687d/\d+$")]
    private static partial Regex SupportedChip();
}

/// LHM reads each control's starting value from settings, and writes it on the first change to
/// software mode. The default is 0, which must never reach a pump, so it starts at 100 instead.
public sealed partial class SeededSettings : ISettings
{
    public bool Contains(string name) => false;

    public void SetValue(string name, string value)
    {
    }

    public string GetValue(string name, string value) => ControlValue().IsMatch(name) ? "100" : value;

    public void Remove(string name)
    {
    }

    // LHM's key is the sensor id + "control" + "value": /lpc/nct6687d/0/control/0/control/value
    [GeneratedRegex(@"/control/\d+/control/value$")]
    private static partial Regex ControlValue();
}

/// Reads registers from a chip's `GetReport`, the only public way to read them.
public static partial class RegisterReport
{
    private const int HeaderCount = 8;
    private const int CommandColumn = 8; // 0xA28 is the first PWM command register

    public static Original? Nct6687D(string report, int index)
    {
        if (index is < 0 or >= HeaderCount) return null;
        if (ChipId().Matches(report) is not { Count: 1 } ids || ids[0].Groups[1].Value != "D592") return null;
        if (Row(report, "0A00") is not { } mode || Row(report, "0A20") is not { } command) return null;
        return new Original((byte)(mode[0] & (1 << index)), command[CommandColumn + index]);
    }

    private static byte[]? Row(string report, string address)
    {
        var rows = RowPattern().Matches(report).Where(m => m.Groups[1].Value == address).ToList();
        if (rows.Count != 1) return null;
        var cells = rows[0].Groups[2].Value.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        var bytes = new byte[cells.Length];
        for (var i = 0; i < cells.Length; i++)
            if (!byte.TryParse(cells[i], System.Globalization.NumberStyles.HexNumber, null, out bytes[i]))
                return null;
        return bytes.Length == 16 ? bytes : null;
    }

    [GeneratedRegex(@"Chip Id: 0x([0-9A-F]+)")]
    private static partial Regex ChipId();

    [GeneratedRegex(@"^\s*([0-9A-F]{4})\s+(.+?)\s*$", RegexOptions.Multiline)]
    private static partial Regex RowPattern();
}
