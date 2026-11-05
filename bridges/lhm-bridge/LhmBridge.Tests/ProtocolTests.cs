using System.Text.Json;
using LhmBridge;
using Xunit;

namespace LhmBridge.Tests;

public sealed class ProtocolTests : IDisposable
{
    private readonly TempDir _dir = new();
    private readonly FakeClock _clock = new();
    private readonly FakeHardware _hw = new(FakeHardware.Case());
    private readonly Protocol _protocol;

    public ProtocolTests() =>
        _protocol = new Protocol(new Bridge(_hw, new OriginalsStore(_dir.File), () => _clock.Now, _clock.Sleep));

    public void Dispose() => _dir.Dispose();

    private JsonElement Send(string line) => JsonDocument.Parse(_protocol.Handle(line)).RootElement;

    private static string FanId => FakeHardware.Case().Id;

    [Fact]
    public void Hello_reports_the_protocol_pin_and_controllable_fans()
    {
        var r = Send("""{"id":1,"op":"hello","protocol":1}""");
        Assert.True(r.GetProperty("ok").GetBoolean());
        Assert.Equal(1, r.GetProperty("id").GetInt64());
        Assert.Equal(Protocol.Version, r.GetProperty("protocol").GetInt32());
        Assert.Equal(LhmInternals.PinnedVersion, r.GetProperty("lhm").GetString());
        Assert.Equal(FanId, r.GetProperty("controllable")[0].GetString());
    }

    [Fact]
    public void Hello_with_another_protocol_is_refused()
    {
        var r = Send("""{"id":1,"op":"hello","protocol":99}""");
        Assert.False(r.GetProperty("ok").GetBoolean());
        Assert.Contains("protocol", r.GetProperty("error").GetString());
    }

    [Fact]
    public void Set_then_restore_reach_the_hardware()
    {
        Assert.True(Send($$"""{"id":2,"op":"set","fan":"{{FanId}}","duty":60}""").GetProperty("ok").GetBoolean());
        Assert.Equal($"write {FanId} 60", _hw.Log[0]);
        Assert.True(Send($$"""{"id":3,"op":"restore","fan":"{{FanId}}"}""").GetProperty("ok").GetBoolean());
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public void A_refused_set_comes_back_as_an_error_with_its_id()
    {
        var r = Send($$"""{"id":7,"op":"set","fan":"{{FanId}}","duty":500}""");
        Assert.False(r.GetProperty("ok").GetBoolean());
        Assert.Equal(7, r.GetProperty("id").GetInt64());
        Assert.Contains("duty", r.GetProperty("error").GetString());
    }

    [Fact]
    public void Snapshot_carries_readings_and_degraded_headers()
    {
        _hw.StuckManual.Add(FanId);
        Send($$"""{"id":1,"op":"set","fan":"{{FanId}}","duty":60}""");
        Send($$"""{"id":2,"op":"restore","fan":"{{FanId}}"}""");
        var r = Send("""{"id":3,"op":"snapshot"}""");
        Assert.True(r.GetProperty("ok").GetBoolean());
        Assert.Equal("cpu", r.GetProperty("sensors")[0].GetProperty("kind").GetString());
        Assert.True(r.GetProperty("degraded").TryGetProperty(FanId, out _));
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("[]")]
    [InlineData("""{"op":"heartbeat"}""")]
    [InlineData("""{"id":"x","op":"heartbeat"}""")]
    [InlineData("""{"id":1}""")]
    [InlineData("""{"id":1,"op":"nope"}""")]
    [InlineData("""{"id":1,"op":"set"}""")]
    [InlineData("""{"id":1,"op":"set","fan":5,"duty":10}""")]
    [InlineData("""{"id":1,"op":"set","fan":"a","duty":"10"}""")]
    [InlineData("""{"id":1,"op":"set","fan":"a","duty":10.5}""")]
    [InlineData("""{"id":1,"op":"restore"}""")]
    public void Malformed_requests_get_an_error_and_write_nothing(string line)
    {
        Assert.False(Send(line).GetProperty("ok").GetBoolean());
        Assert.Empty(_hw.Log);
    }

    [Fact]
    public void Any_valid_request_counts_as_a_heartbeat()
    {
        var bridge = new Bridge(_hw, new OriginalsStore(_dir.File), () => _clock.Now, _clock.Sleep);
        var protocol = new Protocol(bridge);
        _clock.Now += TimeSpan.FromSeconds(4);
        protocol.Handle("""{"id":1,"op":"heartbeat"}""");
        _clock.Now += TimeSpan.FromSeconds(4);
        Assert.False(bridge.CheckWatchdog());
    }

    [Fact]
    public void Garbage_does_not_count_as_a_heartbeat()
    {
        var bridge = new Bridge(_hw, new OriginalsStore(_dir.File), () => _clock.Now, _clock.Sleep);
        var protocol = new Protocol(bridge);
        _clock.Now += TimeSpan.FromSeconds(4);
        protocol.Handle("not json");
        _clock.Now += TimeSpan.FromSeconds(1);
        Assert.True(bridge.CheckWatchdog());
    }

    [Fact]
    public void Shutdown_hands_back_and_asks_the_host_to_stop()
    {
        Send($$"""{"id":1,"op":"set","fan":"{{FanId}}","duty":60}""");
        var r = Send("""{"id":2,"op":"shutdown"}""");
        Assert.True(r.GetProperty("ok").GetBoolean());
        Assert.True(_protocol.ShutdownRequested);
        Assert.False(_hw.IsManual(FakeHardware.Case()));
    }

    [Fact]
    public void Shutdown_reports_a_failed_hand_back()
    {
        Send($$"""{"id":1,"op":"set","fan":"{{FanId}}","duty":60}""");
        _hw.StuckManual.Add(FanId);
        var r = Send("""{"id":2,"op":"shutdown"}""");
        Assert.False(r.GetProperty("ok").GetBoolean());
        Assert.True(_protocol.ShutdownRequested);
    }
}
