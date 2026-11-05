using LibreHardwareMonitor.Hardware;

namespace LhmBridge;

/// The only code that touches LHM. Not covered by CI: it needs the chip, so it is checked by hand
/// on real hardware (README.md).
public sealed class LhmHardware : IFanHardware
{
    private sealed record Slot(IHardware Chip, ISensor Control);

    private readonly Computer _computer;
    private readonly Dictionary<string, Slot> _slots = [];
    private readonly string? _cpuSensor;

    public LhmHardware(Computer computer)
    {
        _computer = computer;
        UpdateAll();
        var headers = new List<Header>();
        foreach (var chip in All().Where(h => h.HardwareType == HardwareType.SuperIO))
        {
            var supported = HeaderRules.IsSupportedChip(chip.Identifier.ToString())
                && chip.Name == "Nuvoton NCT6687D";
            foreach (var control in chip.Sensors.Where(s => s.SensorType == SensorType.Control && s.Control is not null))
            {
                // Refuse rather than guess if a fan and control at one index don't have one name.
                var fan = chip.Sensors.FirstOrDefault(s => s.SensorType == SensorType.Fan && s.Index == control.Index);
                if (fan is null || fan.Name != control.Name) continue;
                var header = new Header(
                    HeaderRules.FanId(fan.Identifier.ToString()),
                    fan.Name,
                    HeaderRules.KindOf(fan.Name),
                    ChipLabel(chip),
                    control.Index,
                    supported);
                headers.Add(header);
                _slots[header.Id] = new Slot(chip, control);
            }
        }
        Headers = headers;
        var temperatures = All()
            .SelectMany(h => h.Sensors.Where(s => s.SensorType == SensorType.Temperature)
                .Select(s => (s.Identifier.ToString(), h.HardwareType, s.Name)))
            .ToList();
        _cpuSensor = HeaderRules.CpuSensor(temperatures);
    }

    public IReadOnlyList<Header> Headers { get; }

    public Readings Read()
    {
        UpdateAll();
        var sensors = new List<SensorDto>();
        var fans = new List<FanDto>();
        var metrics = new List<MetricDto>();
        foreach (var hardware in All())
        {
            foreach (var s in hardware.Sensors)
            {
                if (s.Value is not { } value || float.IsNaN(value)) continue; // missing is never reported as 0
                var id = HeaderRules.FanId(s.Identifier.ToString());
                switch (s.SensorType)
                {
                    case SensorType.Temperature:
                        var kind = s.Identifier.ToString() == _cpuSensor ? "cpu" : "other";
                        sensors.Add(new SensorDto(id, s.Name, kind, value));
                        break;
                    case SensorType.Fan when hardware.HardwareType == HardwareType.SuperIO:
                        var header = Headers.FirstOrDefault(h => h.Id == id);
                        var duty = header is null ? null : _slots[header.Id].Control.Value;
                        var fanKind = HeaderRules.KindOf(s.Name) == HeaderKind.Pump ? "pump" : "fan";
                        fans.Add(new FanDto(id, s.Name, fanKind, value, duty, header?.Controllable ?? false));
                        break;
                    case SensorType.Load when hardware.HardwareType == HardwareType.Cpu && s.Name == "CPU Total":
                        metrics.Add(new MetricDto(id, "cpu_load", value));
                        break;
                    case SensorType.Power when hardware.HardwareType == HardwareType.Cpu && s.Name is "Package" or "CPU Package":
                        metrics.Add(new MetricDto(id, "cpu_power", value));
                        break;
                }
            }
        }
        return new Readings(sensors, fans, metrics);
    }

    public Original? ReadRegisters(Header h)
    {
        if (!h.Controllable) return null;
        var report = _slots[h.Id].Chip.GetReport();
        return report is null ? null : RegisterReport.Nct6687D(report, h.Index);
    }

    public bool? IsManual(Header h) => ReadRegisters(h)?.Mode is { } mode ? mode != 0 : null;

    public float StoredDuty(Header h) => _slots[h.Id].Control.Control!.SoftwareValue;

    public float? ActualDuty(Header h)
    {
        var slot = _slots[h.Id];
        slot.Chip.Update();
        return slot.Control.Value;
    }

    public void Write(Header h, byte duty) => _slots[h.Id].Control.Control!.SetSoftware(duty);

    public void Arm(Header h, Original o) => LhmInternals.Arm(LhmInternals.SuperIoOf(_slots[h.Id].Chip), h.Index, o);

    public void Default(Header h) => _slots[h.Id].Control.Control!.SetDefault();

    private static string ChipLabel(IHardware chip) => chip.Name.Split(' ').Last();

    private IEnumerable<IHardware> All() => _computer.Hardware.SelectMany(Walk);

    private static IEnumerable<IHardware> Walk(IHardware h) => h.SubHardware.SelectMany(Walk).Prepend(h);

    private void UpdateAll()
    {
        foreach (var h in All()) h.Update();
    }
}
