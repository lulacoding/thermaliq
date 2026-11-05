using LhmBridge;
using Xunit;

namespace LhmBridge.Tests;

public sealed class BridgeTests : IDisposable
{
    private readonly TempDir _dir = new();
    private readonly FakeClock _clock = new();

    public void Dispose() => _dir.Dispose();

    private Bridge Make(FakeHardware hw) =>
        new(hw, new OriginalsStore(_dir.File), () => _clock.Now, _clock.Sleep);

    private OriginalsStore Store() => new(_dir.File);

    // Pump rule

    [Theory]
    [InlineData(0)]
    [InlineData(19)]
    public void A_pump_is_never_written_below_the_minimum(int duty)
    {
        var hw = new FakeHardware(FakeHardware.Pump());
        var error = Make(hw).Set(FakeHardware.Pump().Id, duty);
        Assert.Contains("pump", error);
        Assert.Empty(hw.Log);
        Assert.False(Store().Has(FakeHardware.Pump().Id));
    }

    [Fact]
    public void A_pump_at_the_minimum_is_allowed()
    {
        var hw = new FakeHardware(FakeHardware.Pump());
        Assert.Null(Make(hw).Set(FakeHardware.Pump().Id, Bridge.PumpMinDuty));
    }

    [Theory]
    [InlineData(0f)]
    [InlineData(19f)]
    public void A_first_takeover_refuses_if_the_library_would_write_a_low_stored_value(float stored)
    {
        // LHM writes its stored value for about 100 ms before the requested duty.
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        hw.Stored[h.Id] = stored;
        Assert.Contains("stored", Make(hw).Set(h.Id, 60));
        Assert.Empty(hw.Log);
    }

    [Fact]
    public void A_pump_retake_refuses_if_the_stored_value_is_low()
    {
        var p = FakeHardware.Pump();
        var hw = new FakeHardware(p);
        var b = Make(hw);
        Assert.Null(b.Set(p.Id, 50));
        Assert.Null(b.Restore(p.Id));
        hw.Stored[p.Id] = 0;
        hw.Log.Clear();
        Assert.Contains("stored", b.Set(p.Id, 50));
        Assert.Empty(hw.Log);
    }

    [Fact]
    public void A_case_fan_may_stop_and_be_retaken()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 0));
        Assert.Null(b.Restore(h.Id));
        Assert.Null(b.Set(h.Id, 60));
    }

    // Per-chip rule

    [Fact]
    public void Another_chip_is_refused_and_nothing_is_saved_or_written()
    {
        var h = FakeHardware.OtherChip();
        var hw = new FakeHardware(h);
        Assert.Contains("NCT6798D", Make(hw).Set(h.Id, 60));
        Assert.Empty(hw.Log);
        Assert.False(Store().Has(h.Id));
    }

    [Fact]
    public void Controllable_lists_only_supported_headers()
    {
        var hw = new FakeHardware(FakeHardware.Case(), FakeHardware.OtherChip());
        var hello = Make(hw).Hello();
        Assert.Equal([FakeHardware.Case().Id], hello.Controllable);
    }

    // Persistence

    [Fact]
    public void Originals_are_saved_before_the_first_write()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Null(Make(hw).Set(h.Id, 60));
        // The value before our write, not after it. A_failed_save_means_no_write proves the order.
        Assert.Equal(new Original(0, 0x66), Store().Get(h.Id)!.ToOriginal());
        Assert.Equal($"write {h.Id} 60", hw.Log[0]);
    }

    [Fact]
    public void A_failed_save_means_no_write()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Directory.Delete(_dir.Path, true);
        try
        {
            Assert.Contains("save", b.Set(h.Id, 60));
            Assert.Empty(hw.Log);
        }
        finally
        {
            Directory.CreateDirectory(_dir.Path);
        }
    }

    [Fact]
    public void An_existing_record_is_never_overwritten_by_a_takeover()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        hw.StuckManual.Add(h.Id);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        Assert.NotNull(b.Restore(h.Id));
        hw.Registers[h.Id] = new Original(0x20, 0x99);
        Assert.Null(b.Set(h.Id, 70));
        Assert.Equal(new Original(0, 0x66), Store().Get(h.Id)!.ToOriginal());
    }

    [Fact]
    public void A_header_already_manual_with_no_record_is_refused_and_degraded()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        hw.Registers[h.Id] = new Original(0x20, 0x66);
        var b = Make(hw);
        Assert.Contains("already", b.Set(h.Id, 60));
        Assert.Empty(hw.Log);
        Assert.False(Store().Has(h.Id));
        Assert.Contains(h.Id, b.Degraded.Keys);
    }

    [Fact]
    public void A_manual_header_message_names_the_tools_to_close_and_clears_once_it_is_free()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        hw.Registers[h.Id] = new Original(0x20, 0x66);
        var b = Make(hw);
        b.Set(h.Id, 60);
        Assert.Contains("Fan Control", b.Degraded[h.Id]);
        Assert.Contains("MSI Center", b.Degraded[h.Id]);
        hw.Registers[h.Id] = new Original(0, 0x66);
        Assert.Null(b.Set(h.Id, 60));
        Assert.Empty(b.Degraded);
    }

    [Fact]
    public void Unreadable_registers_refuse_the_takeover()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h) { ReportUnreadable = true };
        Assert.Contains("register", Make(hw).Set(h.Id, 60));
        Assert.Empty(hw.Log);
    }

    [Fact]
    public void A_damaged_originals_file_refuses_all_control_and_is_kept()
    {
        var h = FakeHardware.Case();
        File.WriteAllText(_dir.File, "not json");
        var hw = new FakeHardware(h);
        var b = Make(hw);
        b.Recover();
        Assert.Contains("originals", b.Set(h.Id, 60));
        Assert.Empty(hw.Log);
        Assert.Equal("not json", File.ReadAllText(_dir.File));
        Assert.NotEmpty(b.Degraded);
    }

    // Hand-back and read-back

    [Fact]
    public void Hand_back_arms_the_saved_values_then_calls_default_and_drops_the_record()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.Log.Clear();
        Assert.Null(b.Restore(h.Id));
        Assert.Equal([$"arm {h.Id} 0 102", $"default {h.Id}"], hw.Log);
        Assert.False(hw.IsManual(h));
        Assert.False(Store().Has(h.Id));
        Assert.Empty(b.Degraded);
    }

    [Fact]
    public void A_dropped_restore_is_retried_because_lhm_ignores_a_repeated_default()
    {
        // LHM's ISA bus wait can drop the write silently, and a second SetDefault does nothing.
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.DefaultsUntilClear = 1;
        hw.Log.Clear();
        Assert.Null(b.Restore(h.Id));
        Assert.Equal(
            [$"arm {h.Id} 0 102", $"default {h.Id}", $"write {h.Id} 100", $"arm {h.Id} 0 102", $"default {h.Id}"],
            hw.Log);
        Assert.Empty(b.Degraded);
    }

    [Fact]
    public void A_header_still_manual_after_restore_reports_degraded_and_keeps_its_record()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        Assert.Contains("manual", b.Restore(h.Id));
        Assert.Contains(h.Id, b.Degraded.Keys);
        Assert.True(Store().Has(h.Id));
    }

    [Fact]
    public void A_stuck_header_ends_at_100_percent_and_says_so()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        Assert.Contains("100%", b.Restore(h.Id));
        Assert.Equal(100, hw.Actual[h.Id]);
    }

    [Fact]
    public void A_stuck_pump_ends_at_100_percent_too()
    {
        var p = FakeHardware.Pump();
        var hw = new FakeHardware(p);
        var b = Make(hw);
        Assert.Null(b.Set(p.Id, 60));
        hw.StuckManual.Add(p.Id);
        Assert.NotNull(b.Restore(p.Id));
        Assert.Equal(100, hw.Actual[p.Id]);
    }

    [Fact]
    public void A_stuck_header_stays_at_100_percent_when_lhm_closes()
    {
        // LHM's Close restores every control from its save fields. With the old duty still armed,
        // the flag stays set and the fan drops to that duty as the bridge exits.
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        Assert.NotNull(b.Restore(h.Id));
        Assert.Equal(new Original((byte)(1 << h.Index), 0xFF), hw.Armed[h.Id]);
        Assert.Equal(new Original(0, 0x66), Store().Get(h.Id)!.ToOriginal());
    }

    [Fact]
    public void A_stuck_header_that_also_refuses_100_percent_says_that_instead()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        hw.DropWrites = true;
        var error = b.Restore(h.Id);
        Assert.Contains("did not land", error);
        Assert.Equal(error, b.Degraded[h.Id]);
    }

    [Fact]
    public void A_stuck_header_recovered_by_a_restart_also_ends_at_100_percent()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Null(Make(hw).Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        var restarted = Make(hw);
        restarted.Recover();
        Assert.Equal(100, hw.Actual[h.Id]);
        Assert.Contains("100%", restarted.Degraded[h.Id]);
    }

    [Fact]
    public void An_unknown_read_back_is_not_treated_as_success()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.ReportUnreadable = true;
        Assert.NotNull(b.Restore(h.Id));
        Assert.Contains(h.Id, b.Degraded.Keys);
        Assert.True(Store().Has(h.Id));
    }

    [Fact]
    public void Degraded_clears_when_a_later_restore_verifies()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        Assert.NotNull(b.Restore(h.Id));
        hw.StuckManual.Clear();
        Assert.Null(b.Restore(h.Id));
        Assert.Empty(b.Degraded);
    }

    [Fact]
    public void Restoring_a_header_never_taken_writes_nothing()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Null(Make(hw).Restore(h.Id));
        Assert.Empty(hw.Log);
    }

    [Fact]
    public void Restore_all_carries_on_past_a_failure()
    {
        var (a, c) = (FakeHardware.Case(5), FakeHardware.Case(6));
        var hw = new FakeHardware(a, c);
        var b = Make(hw);
        Assert.Null(b.Set(a.Id, 60));
        Assert.Null(b.Set(c.Id, 60));
        hw.StuckManual.Add(a.Id);
        b.RestoreAll();
        Assert.False(hw.IsManual(c));
        Assert.True(hw.IsManual(a));
        Assert.Contains(a.Id, b.Degraded.Keys);
    }

    // Restart

    [Fact]
    public void A_restarted_bridge_restores_from_the_saved_originals()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Null(Make(hw).Set(h.Id, 60));
        // The first bridge is killed here with the header manual.
        hw.Log.Clear();
        hw.Armed.Clear();
        var restarted = Make(hw);
        restarted.Recover();
        Assert.Equal([$"arm {h.Id} 0 102", $"default {h.Id}"], hw.Log);
        Assert.False(hw.IsManual(h));
        Assert.False(Store().Has(h.Id));
        Assert.Empty(restarted.Degraded);
    }

    [Fact]
    public void A_failed_recovery_keeps_the_record_and_reports_degraded()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Null(Make(hw).Set(h.Id, 60));
        hw.StuckManual.Add(h.Id);
        var restarted = Make(hw);
        restarted.Recover();
        Assert.True(Store().Has(h.Id));
        Assert.Contains(h.Id, restarted.Degraded.Keys);
    }

    [Fact]
    public void A_record_for_a_missing_header_is_kept_and_reported()
    {
        var h = FakeHardware.Case();
        Assert.Null(Make(new FakeHardware(h)).Set(h.Id, 60));
        var restarted = Make(new FakeHardware());
        restarted.Recover();
        Assert.True(Store().Has(h.Id));
        Assert.Contains(h.Id, restarted.Degraded.Keys);
    }

    // Writes

    [Theory]
    [InlineData(-1)]
    [InlineData(101)]
    public void An_out_of_range_duty_is_refused(int duty)
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        Assert.Contains("duty", Make(hw).Set(h.Id, duty));
        Assert.Empty(hw.Log);
    }

    [Fact]
    public void An_unknown_fan_is_refused()
    {
        Assert.Contains("unknown", Make(new FakeHardware()).Set("zz", 50));
        Assert.Contains("unknown", Make(new FakeHardware()).Restore("zz"));
    }

    [Fact]
    public void A_dropped_write_is_retried_with_a_nudge_because_lhm_ignores_a_repeated_value()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        hw.Actual[h.Id] = 40;
        hw.Log.Clear();
        Assert.Null(b.Set(h.Id, 60));
        Assert.Equal([$"write {h.Id} 60", $"write {h.Id} 61", $"write {h.Id} 60"], hw.Log);
    }

    [Fact]
    public void A_write_that_never_lands_is_an_error_not_a_success()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h) { DropWrites = true };
        Assert.Contains("did not", Make(hw).Set(h.Id, 60));
    }

    [Fact]
    public void A_pump_nudge_never_goes_below_the_minimum()
    {
        var p = FakeHardware.Pump();
        var hw = new FakeHardware(p);
        var b = Make(hw);
        Assert.Null(b.Set(p.Id, Bridge.PumpMinDuty));
        hw.Actual[p.Id] = 90;
        hw.Log.Clear();
        b.Set(p.Id, Bridge.PumpMinDuty);
        Assert.DoesNotContain(hw.Log, line => line.StartsWith("write") && int.Parse(line.Split(' ')[2]) < Bridge.PumpMinDuty);
    }

    // Watchdog (S6)

    [Fact]
    public void The_watchdog_holds_before_five_seconds()
    {
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        var start = _clock.Now;
        b.Beat();
        _clock.Now = start + Bridge.WatchdogLimit - TimeSpan.FromMilliseconds(1);
        Assert.False(b.CheckWatchdog());
        Assert.True(hw.IsManual(h));
    }

    [Fact]
    public void The_watchdog_hands_back_at_five_seconds_with_no_heartbeat()
    {
        Assert.Equal(TimeSpan.FromSeconds(5), Bridge.WatchdogLimit);
        var h = FakeHardware.Case();
        var hw = new FakeHardware(h);
        var b = Make(hw);
        Assert.Null(b.Set(h.Id, 60));
        b.Beat();
        _clock.Now += Bridge.WatchdogLimit;
        Assert.True(b.CheckWatchdog());
        Assert.False(hw.IsManual(h));
    }

    [Fact]
    public void A_heartbeat_resets_the_watchdog()
    {
        var b = Make(new FakeHardware(FakeHardware.Case()));
        _clock.Now += TimeSpan.FromSeconds(4);
        b.Beat();
        _clock.Now += TimeSpan.FromSeconds(4);
        Assert.False(b.CheckWatchdog());
    }
}
