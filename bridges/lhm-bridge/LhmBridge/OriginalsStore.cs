using System.Text.Json;
using System.Text.Json.Nodes;

namespace LhmBridge;

public sealed record OriginalRecord(string Chip, int Index, byte Mode, byte Command)
{
    public Original ToOriginal() => new(Mode, Command);
}

/// Each header's state from before its first takeover, on disk so a restarted bridge can restore it.
/// The format is NCT6687D's: other chips' save fields mean something else (SPEC 5.1).
public sealed class OriginalsStore(string path)
{
    public const string FileName = "bridge-originals.json";
    private const int Format = 1;
    private const int Headers = 8;

    private Dictionary<string, OriginalRecord>? _records;

    public IReadOnlyDictionary<string, OriginalRecord> All => Records();

    public bool Has(string fan) => Records().ContainsKey(fan);

    public OriginalRecord? Get(string fan) => Records().GetValueOrDefault(fan);

    /// Throws `InvalidDataException` on a damaged or foreign file, and leaves the file alone.
    public void Load()
    {
        if (!File.Exists(path))
        {
            _records = [];
            return;
        }
        try
        {
            _records = Parse(File.ReadAllText(path));
        }
        catch (Exception e) when (e is JsonException or FormatException or InvalidOperationException or OverflowException)
        {
            throw new InvalidDataException($"{path}: {e.Message}", e);
        }
    }

    /// A record is never replaced: it holds the only copy of the BIOS's values.
    public void Add(string fan, OriginalRecord record)
    {
        if (Has(fan)) throw new InvalidOperationException($"{fan} already has a saved original");
        var next = new Dictionary<string, OriginalRecord>(Records()) { [fan] = record };
        Save(next);
        _records = next;
    }

    public void Remove(string fan)
    {
        if (!Has(fan)) return;
        var next = new Dictionary<string, OriginalRecord>(Records());
        next.Remove(fan);
        Save(next);
        _records = next;
    }

    private Dictionary<string, OriginalRecord> Records()
    {
        if (_records is null) Load();
        return _records!;
    }

    private static Dictionary<string, OriginalRecord> Parse(string text)
    {
        var root = JsonNode.Parse(text)?.AsObject() ?? throw new InvalidDataException("empty file");
        if (root["format"]?.GetValue<int>() != Format) throw new InvalidDataException("unknown format");
        var headers = root["headers"]?.AsObject() ?? throw new InvalidDataException("no headers");
        var records = new Dictionary<string, OriginalRecord>();
        foreach (var (fan, node) in headers)
        {
            var o = node?.AsObject() ?? throw new InvalidDataException($"{fan}: no record");
            var chip = o["chip"]?.GetValue<string>();
            if (chip != Chips.Nct6687D) throw new InvalidDataException($"{fan}: chip {chip} is not supported");
            var index = o["index"]!.GetValue<int>();
            if (index is < 0 or >= Headers) throw new InvalidDataException($"{fan}: bad index {index}");
            var mode = o["mode"]!.GetValue<int>();
            if (mode != 0 && mode != 1 << index) throw new InvalidDataException($"{fan}: bad mode {mode}");
            var command = o["command"]!.GetValue<int>();
            if (command is < 0 or > 255) throw new InvalidDataException($"{fan}: bad command {command}");
            records[fan] = new OriginalRecord(chip, index, (byte)mode, (byte)command);
        }
        return records;
    }

    private void Save(Dictionary<string, OriginalRecord> records)
    {
        var headers = new JsonObject();
        foreach (var (fan, r) in records)
            headers[fan] = new JsonObject
            {
                ["chip"] = r.Chip,
                ["index"] = r.Index,
                ["mode"] = (int)r.Mode,
                ["command"] = (int)r.Command,
            };
        var root = new JsonObject { ["format"] = Format, ["headers"] = headers };
        var temp = path + ".tmp";
        try
        {
            using (var f = new FileStream(temp, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                f.Write(System.Text.Encoding.UTF8.GetBytes(root.ToJsonString()));
                f.Flush(flushToDisk: true); // on disk before any fan is touched
            }
            File.Move(temp, path, overwrite: true);
        }
        catch
        {
            if (File.Exists(temp)) File.Delete(temp);
            throw;
        }
    }
}
