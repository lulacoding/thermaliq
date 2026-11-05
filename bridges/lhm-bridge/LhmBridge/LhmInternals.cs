using System.Reflection;

namespace LhmBridge;

/// The private parts of the pinned LHM this bridge depends on (SPEC 5.1). Reflection only sets LHM's
/// own save fields: LHM still does the register write in the public `SetDefault`.
/// shortcut: drop this once LHM has a public way to restore saved state (upstream PR, PORTING M12).
public static class LhmInternals
{
    public const string PinnedVersion = "0.9.7-pre742";
    public const string PinnedCommit = "dc51e75";
    public const string ChipTypeName = "LibreHardwareMonitor.Hardware.Motherboard.Lpc.Nct677X";
    public const string HardwareTypeName = "LibreHardwareMonitor.Hardware.Motherboard.SuperIOHardware";

    private const string ModeField = "_initialFanControlMode";
    private const string CommandField = "_initialFanPwmCommand";
    private const string RequiredField = "_restoreDefaultFanControlRequired";
    private const string SuperIoField = "_superIO";
    private const BindingFlags Private = BindingFlags.Instance | BindingFlags.NonPublic;

    /// Empty means this assembly is the one the restore was written and measured against.
    public static IReadOnlyList<string> Check(Assembly lhm)
    {
        var version = lhm.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
        var problems = new List<string>(CheckVersion(version ?? "no version"));
        if (lhm.GetType(ChipTypeName) is { } chip)
            problems.AddRange(CheckFields(chip));
        else
            problems.Add($"type {ChipTypeName} is missing");
        if (lhm.GetType(HardwareTypeName)?.GetField(SuperIoField, Private) is null)
            problems.Add($"{HardwareTypeName}.{SuperIoField} is missing");
        return problems;
    }

    public static IReadOnlyList<string> CheckVersion(string informational) =>
        informational.StartsWith($"{PinnedVersion}+{PinnedCommit}", StringComparison.Ordinal)
            ? []
            : [$"LHM is {informational}, this bridge is pinned to {PinnedVersion}+{PinnedCommit}"];

    public static IReadOnlyList<string> CheckFields(Type chip)
    {
        var problems = new List<string>();
        void Expect(string name, Type type)
        {
            var field = chip.GetField(name, Private);
            if (field is null)
                problems.Add($"field {name} is missing");
            else if (field.FieldType != type)
                problems.Add($"field {name} is {field.FieldType}, expected {type}");
        }
        Expect(ModeField, typeof(byte[]));
        Expect(CommandField, typeof(byte[]));
        Expect(RequiredField, typeof(bool[]));
        return problems;
    }

    public static object SuperIoOf(object superIoHardware) =>
        superIoHardware.GetType().GetField(SuperIoField, Private)?.GetValue(superIoHardware)
        ?? throw new InvalidOperationException("no Super I/O object behind this hardware");

    /// Sets what `SetDefault` will restore. Writes no register.
    public static void Arm(object chip, int index, Original original)
    {
        var type = chip.GetType();
        var mode = (byte[]?)type.GetField(ModeField, Private)?.GetValue(chip);
        var command = (byte[]?)type.GetField(CommandField, Private)?.GetValue(chip);
        var required = (bool[]?)type.GetField(RequiredField, Private)?.GetValue(chip);
        if (mode is null || command is null || required is null)
            throw new InvalidOperationException($"{type} has no save fields");
        if (index < 0 || index >= mode.Length || index >= command.Length || index >= required.Length)
            throw new InvalidOperationException($"header {index} is outside the save fields of {type}");
        mode[index] = original.Mode;
        command[index] = original.Command;
        required[index] = true;
    }
}
