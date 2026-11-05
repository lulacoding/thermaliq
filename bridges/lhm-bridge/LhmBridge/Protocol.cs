using System.Text.Json;
using System.Text.Json.Nodes;

namespace LhmBridge;

/// One JSON line in, one JSON line out. See README.md.
public sealed class Protocol(Bridge bridge)
{
    public const int Version = 1;

    public bool ShutdownRequested { get; private set; }

    public string Handle(string line)
    {
        long? id = null;
        try
        {
            using var doc = JsonDocument.Parse(line);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) return Error(null, "a request must be an object");
            if (!root.TryGetProperty("id", out var idNode) || !idNode.TryGetInt64(out var idValue))
                return Error(null, "a request needs a numeric id");
            id = idValue;
            if (!root.TryGetProperty("op", out var opNode) || opNode.ValueKind != JsonValueKind.String)
                return Error(id, "a request needs an op");

            var op = opNode.GetString();
            if (op is not ("hello" or "heartbeat" or "snapshot" or "set" or "restore" or "shutdown"))
                return Error(id, $"unknown op {op}");
            bridge.Beat();

            return op switch
            {
                "hello" => HandleHello(id.Value, root),
                "heartbeat" => Ok(id.Value),
                "snapshot" => HandleSnapshot(id.Value),
                "set" => HandleSet(id.Value, root),
                "restore" => HandleRestore(id.Value, root),
                _ => HandleShutdown(id.Value),
            };
        }
        catch (JsonException e)
        {
            return Error(id, $"bad json: {e.Message}");
        }
        catch (Exception e)
        {
            return Error(id, e.Message);
        }
    }

    private string HandleHello(long id, JsonElement root)
    {
        if (!root.TryGetProperty("protocol", out var p) || !p.TryGetInt32(out var version) || version != Version)
            return Error(id, $"this bridge speaks protocol {Version}");
        var hello = bridge.Hello();
        var reply = OkNode(id);
        reply["protocol"] = Version;
        reply["lhm"] = LhmInternals.PinnedVersion;
        reply["controllable"] = JsonSerializer.SerializeToNode(hello.Controllable);
        reply["refused_chips"] = JsonSerializer.SerializeToNode(hello.RefusedChips);
        return reply.ToJsonString();
    }

    private string HandleSnapshot(long id)
    {
        var reply = OkNode(id);
        var readings = JsonSerializer.SerializeToNode(bridge.Snapshot())!.AsObject();
        foreach (var name in new[] { "sensors", "fans", "metrics" })
            reply[name] = readings[name]!.DeepClone();
        reply["degraded"] = JsonSerializer.SerializeToNode(bridge.Degraded);
        return reply.ToJsonString();
    }

    private string HandleSet(long id, JsonElement root)
    {
        if (Fan(root) is not { } fan) return Error(id, "set needs a fan");
        if (!root.TryGetProperty("duty", out var d) || d.ValueKind != JsonValueKind.Number || !d.TryGetInt32(out var duty))
            return Error(id, "set needs a whole-number duty");
        return Result(id, bridge.Set(fan, duty));
    }

    private string HandleRestore(long id, JsonElement root) =>
        Fan(root) is { } fan ? Result(id, bridge.Restore(fan)) : Error(id, "restore needs a fan");

    private string HandleShutdown(long id)
    {
        ShutdownRequested = true;
        var failed = bridge.RestoreAll();
        return failed == 0 ? Ok(id) : Error(id, $"{failed} fan(s) did not hand back");
    }

    private static string? Fan(JsonElement root) =>
        root.TryGetProperty("fan", out var f) && f.ValueKind == JsonValueKind.String ? f.GetString() : null;

    private static string Result(long id, string? error) => error is null ? Ok(id) : Error(id, error);

    private static string Ok(long id) => OkNode(id).ToJsonString();

    private static JsonObject OkNode(long id) => new() { ["id"] = id, ["ok"] = true };

    private static string Error(long? id, string message) =>
        new JsonObject { ["id"] = id, ["ok"] = false, ["error"] = message }.ToJsonString();
}
