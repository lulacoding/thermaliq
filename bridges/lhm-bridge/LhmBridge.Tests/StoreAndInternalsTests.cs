using System.Reflection;
using System.Runtime.CompilerServices;
using LhmBridge;
using LibreHardwareMonitor.Hardware;
using Xunit;

namespace LhmBridge.Tests;

public sealed class OriginalsStoreTests : IDisposable
{
    private readonly TempDir _dir = new();

    public void Dispose() => _dir.Dispose();

    private static OriginalRecord Record(int index = 5, byte mode = 0, byte command = 0x66) =>
        new(Chips.Nct6687D, index, mode, command);

    [Fact]
    public void A_record_survives_a_new_store_instance()
    {
        var s = new OriginalsStore(_dir.File);
        s.Load();
        s.Add("f", Record());
        var again = new OriginalsStore(_dir.File);
        again.Load();
        Assert.Equal(Record(), again.Get("f"));
    }

    [Fact]
    public void Adding_over_an_existing_record_is_refused()
    {
        var s = new OriginalsStore(_dir.File);
        s.Load();
        s.Add("f", Record());
        Assert.Throws<InvalidOperationException>(() => s.Add("f", Record(command: 1)));
        Assert.Equal(0x66, s.Get("f")!.Command);
    }

    [Fact]
    public void Removing_the_last_record_leaves_nothing_behind_to_recover()
    {
        var s = new OriginalsStore(_dir.File);
        s.Load();
        s.Add("f", Record());
        s.Remove("f");
        var again = new OriginalsStore(_dir.File);
        again.Load();
        Assert.Empty(again.All);
    }

    [Fact]
    public void A_write_leaves_no_temp_file()
    {
        var s = new OriginalsStore(_dir.File);
        s.Load();
        s.Add("f", Record());
        Assert.Equal([_dir.File], Directory.GetFiles(_dir.Path));
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("{}")]
    [InlineData("""{"format":2,"headers":{}}""")]
    [InlineData("""{"format":1,"headers":{"f":{"chip":"NCT6798D","index":5,"mode":0,"command":1}}}""")]
    [InlineData("""{"format":1,"headers":{"f":{"chip":"NCT6687DR","index":5,"mode":0,"command":1}}}""")]
    [InlineData("""{"format":1,"headers":{"f":{"chip":"NCT6687D","index":8,"mode":0,"command":1}}}""")]
    [InlineData("""{"format":1,"headers":{"f":{"chip":"NCT6687D","index":5,"mode":1,"command":1}}}""")]
    [InlineData("""{"format":1,"headers":{"f":{"chip":"NCT6687D","index":5,"mode":32,"command":300}}}""")]
    public void A_damaged_or_foreign_file_is_refused(string content)
    {
        File.WriteAllText(_dir.File, content);
        Assert.Throws<InvalidDataException>(() => new OriginalsStore(_dir.File).Load());
        Assert.Equal(content, File.ReadAllText(_dir.File));
    }

    [Fact]
    public void A_missing_file_is_an_empty_store()
    {
        var s = new OriginalsStore(_dir.File);
        s.Load();
        Assert.Empty(s.All);
    }
}

public sealed class RegisterReportTests
{
    // Layout from a real pre742 dump on NCT6687D (registers only).
    private const string Dump = """
        LPC Nct677X

        Chip Id: 0xD592
        Chip Revision: 0x92
        Base Address: 0x0A20

        Hardware Monitor Registers

                00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F

         0000   D5 92 1C 00 00 00 00 03 FF 00 FC 83 01 00 01 F8
         0A00   20 00 00 00 00 00 00 00 40 40 40 40 40 40 40 40
         0A20   00 00 00 00 00 00 00 00 66 66 66 66 66 99 66 67
        """;

    [Fact]
    public void Reads_the_mode_bit_and_command_of_a_header()
    {
        Assert.Equal(new Original(0x20, 0x99), RegisterReport.Nct6687D(Dump, 5));
        Assert.Equal(new Original(0, 0x67), RegisterReport.Nct6687D(Dump, 7));
        Assert.Equal(new Original(0, 0x66), RegisterReport.Nct6687D(Dump, 0));
    }

    [Fact]
    public void A_report_cut_short_gives_no_answer()
    {
        // LHM returns only the header when it can't get the ISA bus in time.
        Assert.Null(RegisterReport.Nct6687D("LPC Nct677X\n\nChip Id: 0xD592\n", 5));
        Assert.Null(RegisterReport.Nct6687D(Dump.Replace("0A20 ", "0A30 "), 5));
    }

    [Fact]
    public void Another_chip_id_gives_no_answer()
    {
        Assert.Null(RegisterReport.Nct6687D(Dump.Replace("0xD592", "0xD428"), 5));
    }

    [Fact]
    public void Two_chips_in_one_report_give_no_answer()
    {
        Assert.Null(RegisterReport.Nct6687D(Dump + "\n" + Dump, 5));
    }

    [Theory]
    [InlineData(-1)]
    [InlineData(8)]
    public void An_out_of_range_index_gives_no_answer(int index)
    {
        Assert.Null(RegisterReport.Nct6687D(Dump, index));
    }

    [Fact]
    public void A_malformed_row_gives_no_answer()
    {
        Assert.Null(RegisterReport.Nct6687D(Dump.Replace(" 0A00   20", " 0A00   ZZ"), 5));
    }
}

public sealed class SelfCheckTests
{
    private static Assembly Lhm => typeof(Computer).Assembly;

    [Fact]
    public void The_pinned_build_passes_against_the_real_assembly()
    {
        Assert.Empty(LhmInternals.Check(Lhm));
    }

    [Fact]
    public void A_different_version_is_refused()
    {
        var problems = LhmInternals.CheckVersion("0.9.7-pre780+abc");
        Assert.Contains(problems, p => p.Contains("0.9.7-pre780"));
        Assert.Empty(LhmInternals.CheckVersion(LhmInternals.PinnedVersion + "+dc51e75bd97b15ce17ded0885e67bad47be0765b"));
    }

    [Fact]
    public void An_assembly_without_the_types_is_refused()
    {
        var problems = LhmInternals.Check(typeof(SelfCheckTests).Assembly);
        Assert.NotEmpty(problems);
    }

    private sealed class WrongTypes
    {
#pragma warning disable CS0649, CS0169
        private int[]? _initialFanControlMode;
        private byte[]? _initialFanPwmCommand;
        private bool[]? _restoreDefaultFanControlRequired;
#pragma warning restore CS0649, CS0169
    }

    private sealed class MissingField
    {
#pragma warning disable CS0649, CS0169
        private byte[]? _initialFanControlMode;
        private byte[]? _initialFanPwmCommand;
#pragma warning restore CS0649, CS0169
    }

    [Fact]
    public void A_field_of_another_type_is_refused()
    {
        Assert.Contains(LhmInternals.CheckFields(typeof(WrongTypes)), p => p.Contains("_initialFanControlMode"));
    }

    [Fact]
    public void A_missing_field_is_refused()
    {
        Assert.Contains(LhmInternals.CheckFields(typeof(MissingField)), p => p.Contains("_restoreDefaultFanControlRequired"));
    }

    [Fact]
    public void Arm_sets_the_real_private_fields_of_the_pinned_type()
    {
        var type = Lhm.GetType(LhmInternals.ChipTypeName)!;
        var chip = RuntimeHelpers.GetUninitializedObject(type);
        void Init(string name, object value) =>
            type.GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)!.SetValue(chip, value);
        byte[] mode = new byte[8], command = new byte[8];
        var required = new bool[8];
        Init("_initialFanControlMode", mode);
        Init("_initialFanPwmCommand", command);
        Init("_restoreDefaultFanControlRequired", required);

        LhmInternals.Arm(chip, 5, new Original(0, 0x66));

        Assert.Equal(0, mode[5]);
        Assert.Equal(0x66, command[5]);
        Assert.True(required[5]);
        Assert.False(required[4]);
    }

    [Fact]
    public void Arm_refuses_an_index_outside_the_arrays()
    {
        var type = Lhm.GetType(LhmInternals.ChipTypeName)!;
        var chip = RuntimeHelpers.GetUninitializedObject(type);
        void Init(string name, object value) =>
            type.GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)!.SetValue(chip, value);
        Init("_initialFanControlMode", new byte[2]);
        Init("_initialFanPwmCommand", new byte[2]);
        Init("_restoreDefaultFanControlRequired", new bool[2]);
        Assert.Throws<InvalidOperationException>(() => LhmInternals.Arm(chip, 5, new Original(0, 1)));
    }
}

public class SensorProxy : DispatchProxy
{
    public Identifier? Id;

    protected override object? Invoke(MethodInfo? targetMethod, object?[]? args) =>
        targetMethod?.Name == "get_Identifier" ? Id : null;
}

/// Against LHM's real internal `Control` class: a hand-written settings key would prove nothing.
public sealed class SeededSettingsTests
{
    public sealed class Recorder(List<float> seen)
    {
        public void Record(object control) => seen.Add(((IControl)control).SoftwareValue);
    }

    private static (IControl Control, List<float> Modes, List<float> Values) RealControl(ISettings settings)
    {
        var sensor = DispatchProxy.Create<ISensor, SensorProxy>();
        ((SensorProxy)(object)sensor).Id = new Identifier("lpc", "nct6687d", "0", "control", "0");
        var type = typeof(Computer).Assembly.GetType("LibreHardwareMonitor.Hardware.Control")!;
        var control = (IControl)Activator.CreateInstance(
            type,
            BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic,
            null,
            [sensor, settings, 0f, 100f],
            null)!;
        List<float> modes = [], values = [];
        void Watch(string name, List<float> seen)
        {
            var e = type.GetEvent(name, BindingFlags.Instance | BindingFlags.NonPublic)!;
            var handler = Delegate.CreateDelegate(e.EventHandlerType!, new Recorder(seen), typeof(Recorder).GetMethod(nameof(Recorder.Record))!);
            e.GetAddMethod(nonPublic: true)!.Invoke(control, [handler]);
        }
        Watch("ControlModeChanged", modes);
        Watch("SoftwareControlValueChanged", values);
        return (control, modes, values);
    }

    [Fact]
    public void The_real_control_starts_at_100_with_the_seed_and_0_without()
    {
        Assert.Equal(100f, RealControl(new SeededSettings()).Control.SoftwareValue);
        Assert.Equal(0f, RealControl(new EmptySettings()).Control.SoftwareValue); // the hazard itself
    }

    [Fact]
    public void The_first_takeover_writes_100_then_the_requested_duty_never_0()
    {
        var (control, modes, values) = RealControl(new SeededSettings());
        control.SetSoftware(60);
        Assert.Equal([100f], modes); // what LHM writes on the mode change
        Assert.Equal([60f], values);
    }

    [Fact]
    public void Default_from_a_fresh_control_fires_so_a_restarted_bridge_can_restore()
    {
        var (control, modes, _) = RealControl(new SeededSettings());
        Assert.Equal(ControlMode.Undefined, control.ControlMode);
        control.SetDefault();
        Assert.Single(modes);
        Assert.Equal(ControlMode.Default, control.ControlMode);
    }

    [Fact]
    public void A_repeated_default_does_nothing_which_is_why_a_retry_leaves_default_first()
    {
        var (control, modes, _) = RealControl(new SeededSettings());
        control.SetDefault();
        control.SetDefault();
        Assert.Single(modes);
        control.SetSoftware(100);
        control.SetDefault();
        Assert.Equal(3, modes.Count);
    }

    [Theory]
    [InlineData("/lpc/nct6687d/0/control/0/control/mode", "0")]
    [InlineData("/lpc/nct6687d/0/fan/0/min", "7")]
    public void Everything_else_keeps_its_default(string key, string fallback)
    {
        Assert.Equal(fallback, new SeededSettings().GetValue(key, fallback));
    }

    [Fact]
    public void Nothing_is_persisted_between_runs()
    {
        var s = new SeededSettings();
        const string key = "/lpc/nct6687d/0/control/0/control/value";
        s.SetValue(key, "5");
        Assert.Equal("100", s.GetValue(key, "0"));
        Assert.False(s.Contains(key));
    }

    private sealed class EmptySettings : ISettings
    {
        public bool Contains(string name) => false;

        public void SetValue(string name, string value)
        {
        }

        public string GetValue(string name, string value) => value;

        public void Remove(string name)
        {
        }
    }
}

public sealed class HeaderRulesTests
{
    [Theory]
    [InlineData("Pump Fan", HeaderKind.Pump)]
    [InlineData("AIO pump", HeaderKind.Pump)]
    [InlineData("CPU Fan", HeaderKind.Fan)]
    [InlineData("System Fan #1", HeaderKind.Fan)]
    public void A_pump_is_recognised_by_name(string name, HeaderKind kind)
    {
        Assert.Equal(kind, HeaderRules.KindOf(name));
    }

    [Theory]
    [InlineData("/lpc/nct6687d/0", true)]
    [InlineData("/lpc/nct6687d/1", true)]
    [InlineData("/lpc/nct6687dr/0", false)]
    [InlineData("/lpc/nct6798d/0", false)]
    [InlineData("/lpc/it8688e/0", false)]
    [InlineData("/nct6687d/0", false)]
    public void Only_nct6687d_is_supported(string identifier, bool supported)
    {
        Assert.Equal(supported, HeaderRules.IsSupportedChip(identifier));
    }

    [Theory]
    [InlineData("/lpc/nct6687d/0/fan/5", "lhm_nct6687d_0_fan_5")]
    [InlineData("/lpc/it8688e/0/fan/1", "lhm_it8688e_0_fan_1")]
    public void Fan_ids_are_stable_text(string identifier, string id)
    {
        Assert.Equal(id, HeaderRules.FanId(identifier));
    }

    [Fact]
    public void Only_one_cpu_sensor_is_picked_and_the_cpu_package_beats_the_board_reading()
    {
        var board = ("/lpc/nct6687d/0/temperature/0", HardwareType.SuperIO, "CPU");
        var package = ("/amdcpu/0/temperature/2", HardwareType.Cpu, "Core (Tctl/Tdie)");
        var core = ("/amdcpu/0/temperature/3", HardwareType.Cpu, "CCD1 (Tdie)");
        Assert.Equal(package.Item1, HeaderRules.CpuSensor([board, core, package]));
        Assert.Equal(board.Item1, HeaderRules.CpuSensor([board, core]));
        Assert.Null(HeaderRules.CpuSensor([core]));
    }

    [Fact]
    public void An_intel_package_counts_too()
    {
        var package = ("/intelcpu/0/temperature/5", HardwareType.Cpu, "CPU Package");
        Assert.Equal(package.Item1, HeaderRules.CpuSensor([package]));
    }
}
