using System.Diagnostics;

namespace LhmBridge;

public sealed record HelloInfo(IReadOnlyList<string> Controllable, IReadOnlyList<string> RefusedChips);

/// Every fan rule, with no pipe and no LHM. All methods take one lock, so the watchdog can't
/// interleave with a request. Error strings are returned, null means done and verified (S8).
public sealed class Bridge
{
    /// Matches the engine's `PUMP_MIN_DUTY` (placeholder until M20d).
    public const int PumpMinDuty = 20;
    public static readonly TimeSpan WatchdogLimit = TimeSpan.FromSeconds(5);

    private const int RestoreAttempts = 3;
    private const int DutyTolerance = 3;
    private const int DutyPolls = 5;
    private const string StoreKey = "originals";
    private static readonly TimeSpan DutyPollGap = TimeSpan.FromMilliseconds(100);

    private readonly object _gate = new();
    private readonly IFanHardware _hw;
    private readonly OriginalsStore _store;
    private readonly Func<TimeSpan> _now;
    private readonly Action<TimeSpan> _sleep;
    private readonly HashSet<string> _taken = [];
    private readonly HashSet<string> _everTaken = [];
    private readonly Dictionary<string, string> _degraded = [];
    private TimeSpan _lastBeat;
    private int _slowestLanding;

    public Bridge(IFanHardware hw, OriginalsStore store)
        : this(hw, store, Elapsed(), Thread.Sleep)
    {
    }

    public Bridge(IFanHardware hw, OriginalsStore store, Func<TimeSpan> now, Action<TimeSpan> sleep)
    {
        (_hw, _store, _now, _sleep) = (hw, store, now, sleep);
        _lastBeat = now();
    }

    public IReadOnlyDictionary<string, string> Degraded
    {
        get
        {
            lock (_gate) return new Dictionary<string, string>(_degraded);
        }
    }

    public HelloInfo Hello()
    {
        lock (_gate)
        {
            var controllable = _hw.Headers.Where(h => h.Controllable).Select(h => h.Id).ToList();
            var refused = _hw.Headers.Where(h => !h.Controllable).Select(h => h.Chip).Distinct().ToList();
            return new HelloInfo(controllable, refused);
        }
    }

    public Readings Snapshot()
    {
        lock (_gate) return _hw.Read();
    }

    public void Beat()
    {
        lock (_gate) _lastBeat = _now();
    }

    /// True once 5 s have passed with no heartbeat: every fan has been handed back (S6).
    public bool CheckWatchdog()
    {
        lock (_gate)
        {
            if (_now() - _lastBeat < WatchdogLimit) return false;
            RestoreAll();
            return true;
        }
    }

    /// Hands back whatever a previous bridge left behind. Run once, before serving requests.
    public void Recover()
    {
        lock (_gate)
        {
            try
            {
                _store.Load();
            }
            catch (InvalidDataException e)
            {
                _degraded[StoreKey] = e.Message;
                return;
            }
            foreach (var fan in _store.All.Keys.ToList())
            {
                var error = Restore(fan);
                if (error is not null) _degraded[fan] = error;
            }
        }
    }

    public string? Set(string fan, int duty)
    {
        lock (_gate)
        {
            if (Find(fan) is not { } h) return $"unknown fan {fan}";
            if (!h.Controllable) return $"chip {h.Chip} is refused: only {Chips.Nct6687D} has been measured";
            if (duty is < 0 or > 100) return $"duty {duty} is outside 0-100";
            if (h.Kind == HeaderKind.Pump && duty < PumpMinDuty)
                return $"pump {fan} may not go below {PumpMinDuty}%";
            if (!_taken.Contains(fan) && TakeOver(h) is { } refused) return refused;

            _hw.Write(h, (byte)duty);
            if (Landed(h, duty)) return null;
            // LHM ignores a repeated value, so a dropped write needs a different one first.
            _hw.Write(h, (byte)(duty == 100 ? 99 : duty + 1));
            _hw.Write(h, (byte)duty);
            return Landed(h, duty) ? null : $"write to {fan} did not land: wanted {duty}%, chip reads {_hw.ActualDuty(h)}%";
        }
    }

    /// Hands one header back, verified by reading the chip. Its record stays until then.
    public string? Restore(string fan)
    {
        lock (_gate)
        {
            var header = Find(fan);
            var record = TryGetRecord(fan);
            if (header is null)
            {
                if (record is null) return $"unknown fan {fan}";
                return _degraded[fan] = $"saved original for {fan}, but no such header now";
            }
            if (record is null) return _taken.Contains(fan) ? _degraded[fan] = $"{fan} has no saved original" : null;
            if (!header.Controllable) return _degraded[fan] = $"saved original for {fan}, but its chip is refused";

            for (var attempt = 0; attempt < RestoreAttempts; attempt++)
            {
                // LHM ignores a repeated default, so a dropped restore has to leave default mode
                // first. 100% is the safe direction if this fails half way.
                if (attempt > 0) _hw.Write(header, 100);
                _hw.Arm(header, record.ToOriginal());
                _hw.Default(header);
                if (_hw.IsManual(header) == false)
                {
                    _store.Remove(fan);
                    _taken.Remove(fan);
                    _degraded.Remove(fan);
                    return null;
                }
            }
            return _degraded[fan] = Hold(header);
        }
    }

    /// The BIOS wouldn't take the header back, so nothing else will cool with it: 100%, per header
    /// (SPEC §5.1). Its record stays, so a later restore or a restarted bridge can still hand it back.
    private string Hold(Header h)
    {
        _hw.Write(h, 100);
        var held = Landed(h, 100);
        // Armed last: LHM's Close restores from these fields, and the old duty would end the 100%.
        _hw.Arm(h, new Original((byte)(1 << h.Index), 0xFF));
        return $"{h.Id} is still manual after hand-back, or its registers can't be read: "
            + (held ? "holding it at 100%" : "the 100% write did not land either");
    }

    /// Never throws: one bad header must not leave the others on a bridge duty. Returns the failures.
    public int RestoreAll()
    {
        lock (_gate)
        {
            var failed = 0;
            var fans = _taken.Concat(TryRecords()).Distinct().ToList();
            foreach (var fan in fans)
            {
                try
                {
                    if (Restore(fan) is not null) failed++;
                }
                catch (Exception e)
                {
                    _degraded[fan] = e.Message;
                    failed++;
                }
            }
            return failed;
        }
    }

    private string? TakeOver(Header h)
    {
        // A case fan may legitimately be left at 0 and retaken. A first takeover and a pump may not.
        var stored = _hw.StoredDuty(h);
        if ((!_everTaken.Contains(h.Id) || h.Kind == HeaderKind.Pump) && stored < PumpMinDuty)
            return $"refusing {h.Id}: LHM would first write its stored value {stored}%";
        try
        {
            if (!_store.Has(h.Id))
            {
                if (_hw.ReadRegisters(h) is not { } now)
                    return $"cannot read the registers of {h.Id}, so its original can't be saved";
                if (now.Mode != 0)
                    return _degraded[h.Id] = $"{h.Id} is already manual and we hold no record of it, so another fan tool probably owns it. Close Fan Control or MSI Center";
                _store.Add(h.Id, new OriginalRecord(h.Chip, h.Index, now.Mode, now.Command));
                _degraded.Remove(h.Id);
            }
        }
        catch (InvalidDataException e)
        {
            return _degraded[StoreKey] = $"originals file is damaged: {e.Message}";
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return $"cannot save the original of {h.Id}: {e.Message}";
        }
        _everTaken.Add(h.Id);
        _taken.Add(h.Id);
        return null;
    }

    private bool Landed(Header h, int duty)
    {
        for (var poll = 0; poll < DutyPolls; poll++)
        {
            if (_hw.ActualDuty(h) is { } actual && Math.Abs(actual - duty) <= DutyTolerance)
            {
                // Only a new worst, so the log stays small on a service that runs for months.
                if (poll > _slowestLanding) Console.Error.WriteLine($"a write took {_slowestLanding = poll} read-back polls to land");
                return true;
            }
            _sleep(DutyPollGap);
        }
        return false;
    }

    private Header? Find(string fan) => _hw.Headers.FirstOrDefault(h => h.Id == fan);

    private OriginalRecord? TryGetRecord(string fan)
    {
        try
        {
            return _store.Get(fan);
        }
        catch (InvalidDataException e)
        {
            _degraded[StoreKey] = e.Message;
            return null;
        }
    }

    private IEnumerable<string> TryRecords()
    {
        try
        {
            return _store.All.Keys.ToList();
        }
        catch (InvalidDataException e)
        {
            _degraded[StoreKey] = e.Message;
            return [];
        }
    }

    private static Func<TimeSpan> Elapsed()
    {
        var clock = Stopwatch.StartNew();
        return () => clock.Elapsed;
    }
}
