using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using LhmBridge;
using Xunit;

namespace LhmBridge.Tests;

public sealed class PipeTests : IDisposable
{
    private static readonly TimeSpan Limit = TimeSpan.FromSeconds(20);
    private readonly TempDir _dir = new();
    private readonly FakeHardware _hw = new(FakeHardware.Case());
    private readonly string _name = "lhm-bridge-test-" + Guid.NewGuid().ToString("N");

    public void Dispose() => _dir.Dispose();

    private static PipeSecurity MeOnly()
    {
        var s = new PipeSecurity();
        s.AddAccessRule(new PipeAccessRule(
            WindowsIdentity.GetCurrent().User!,
            PipeAccessRights.ReadWrite | PipeAccessRights.CreateNewInstance | PipeAccessRights.Synchronize,
            AccessControlType.Allow));
        return s;
    }

    private (Bridge Bridge, Task Host, CancellationTokenSource Stop) Start(TimeSpan? tick = null)
    {
        var bridge = new Bridge(_hw, new OriginalsStore(_dir.File));
        var stop = new CancellationTokenSource();
        var server = PipeHost.Create(_name, MeOnly());
        var host = Task.Run(() => PipeHost.Run(server, bridge, tick ?? TimeSpan.FromMilliseconds(20), stop.Token));
        return (bridge, host, stop);
    }

    private NamedPipeClientStream Connect()
    {
        var c = new NamedPipeClientStream(".", _name, PipeDirection.InOut);
        c.Connect((int)Limit.TotalMilliseconds);
        return c;
    }

    private static string Exchange(NamedPipeClientStream c, string line)
    {
        var bytes = Encoding.UTF8.GetBytes(line + "\n");
        c.Write(bytes);
        c.Flush();
        var reply = new List<byte>();
        int b;
        while ((b = c.ReadByte()) is not (-1 or '\n')) reply.Add((byte)b);
        return Encoding.UTF8.GetString(reply.ToArray());
    }

    [Fact]
    public void The_production_acl_allows_only_system()
    {
        var sddl = PipeHost.SystemOnly().GetSecurityDescriptorSddlForm(AccessControlSections.Access);
        var aces = sddl.Split('(', StringSplitOptions.RemoveEmptyEntries).Skip(1).ToArray();
        var ace = Assert.Single(aces);
        Assert.StartsWith("A;", ace);
        Assert.EndsWith(";SY)", ace);
    }

    [Fact]
    public void A_client_who_is_not_system_cannot_open_the_production_pipe()
    {
        Assert.False(WindowsIdentity.GetCurrent().IsSystem, "run these tests as a normal user");
        using var server = PipeHost.Create(_name, PipeHost.SystemOnly());
        using var c = new NamedPipeClientStream(".", _name, PipeDirection.InOut);
        Assert.Throws<UnauthorizedAccessException>(() => c.Connect(2000));
    }

    [Fact]
    public void A_second_server_on_the_same_name_is_refused()
    {
        using var first = PipeHost.Create(_name, MeOnly());
        var e = Record.Exception(() => PipeHost.Create(_name, MeOnly()));
        Assert.True(e is IOException or UnauthorizedAccessException, $"got {e}");
    }

    [Fact]
    public async Task Requests_round_trip_over_json_lines()
    {
        var (_, host, stop) = Start();
        using (var c = Connect())
        {
            var reply = Exchange(c, """{"id":1,"op":"heartbeat"}""");
            Assert.Contains("\"ok\":true", reply);
            stop.Cancel();
        }
        await host.WaitAsync(Limit);
    }

    [Fact]
    public async Task A_disconnect_hands_every_fan_back_at_once()
    {
        var (_, host, _) = Start();
        var id = FakeHardware.Case().Id;
        using (var c = Connect())
        {
            Assert.Contains("\"ok\":true", Exchange(c, $$"""{"id":1,"op":"set","fan":"{{id}}","duty":60}"""));
            Assert.True(_hw.IsManual(FakeHardware.Case()));
        }
        await host.WaitAsync(Limit);
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public async Task Shutdown_stops_the_host()
    {
        var (_, host, _) = Start();
        using var c = Connect();
        Assert.Contains("\"ok\":true", Exchange(c, """{"id":1,"op":"shutdown"}"""));
        await host.WaitAsync(Limit);
    }

    [Fact]
    public async Task An_oversized_line_drops_the_client_and_hands_back()
    {
        var (_, host, _) = Start();
        var id = FakeHardware.Case().Id;
        using var c = Connect();
        Assert.Contains("\"ok\":true", Exchange(c, $$"""{"id":1,"op":"set","fan":"{{id}}","duty":60}"""));
        try
        {
            c.Write(new byte[PipeHost.MaxLineBytes + 1]);
        }
        catch (IOException)
        {
            // The host may hang up before the write ends.
        }
        await host.WaitAsync(Limit);
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public async Task Cancelling_hands_back_too()
    {
        var (_, host, stop) = Start();
        var id = FakeHardware.Case().Id;
        using var c = Connect();
        Assert.Contains("\"ok\":true", Exchange(c, $$"""{"id":1,"op":"set","fan":"{{id}}","duty":60}"""));
        stop.Cancel();
        await host.WaitAsync(Limit);
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public async Task The_real_watchdog_stops_the_host_when_a_connected_client_goes_quiet()
    {
        var clock = new FakeClock();
        var bridge = new Bridge(_hw, new OriginalsStore(_dir.File), () => clock.Now, clock.Sleep);
        var server = PipeHost.Create(_name, MeOnly());
        var host = Task.Run(() => PipeHost.Run(server, bridge, TimeSpan.FromMilliseconds(10), CancellationToken.None));
        var id = FakeHardware.Case().Id;
        using var c = Connect();
        Assert.Contains("\"ok\":true", Exchange(c, $$"""{"id":1,"op":"set","fan":"{{id}}","duty":60}"""));
        clock.Now += Bridge.WatchdogLimit;
        await host.WaitAsync(Limit);
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public async Task No_client_at_all_ends_in_the_watchdog()
    {
        var clock = new FakeClock();
        var bridge = new Bridge(_hw, new OriginalsStore(_dir.File), () => clock.Now, clock.Sleep);
        var server = PipeHost.Create(_name, MeOnly());
        var host = Task.Run(() => PipeHost.Run(server, bridge, TimeSpan.FromMilliseconds(10), CancellationToken.None));
        clock.Now += Bridge.WatchdogLimit;
        await host.WaitAsync(Limit);
    }
}
