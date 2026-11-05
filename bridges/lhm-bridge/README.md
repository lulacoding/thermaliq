# lhm-bridge

.NET sidecar for motherboard fans (SPEC §5.1). The daemon starts it and talks to it over a named pipe. Only the bridge touches LibreHardwareMonitorLib and PawnIO.

**Status:** tests pass with fake hardware and against LHM's real types. It has never run on real hardware. `LhmHardware.cs` is not covered by CI, because it needs the chip. Don't run it on real fans until Sam has reviewed it.

## Build and test

```powershell
dotnet test bridges\lhm-bridge\LhmBridge.sln -c Release
dotnet publish bridges\lhm-bridge\LhmBridge\LhmBridge.csproj -c Release -r win-x64 --self-contained
```

Publish without trimming: the restore depends on private LHM fields, and the startup self-check refuses to run if they are gone.

## Running

`lhm-bridge --pipe <name> --data-dir <dir>`, as SYSTEM. Don't put it in a kill-on-close job: when the daemon dies, the bridge has to live long enough to hand back.

| Exit code | Meaning |
|---|---|
| 0 | Stopped, everything handed back |
| 2 | Bad arguments |
| 10 | PawnIO is not installed: motherboard fans are absent |
| 11 | Self-check failed: LHM isn't the pinned build, or its private fields differ |
| 12 | The pipe name is taken |
| 13 | Failed, or a header could not be handed back |

## Protocol (version 1)

One JSON object per line, in each direction, at most 64 KiB per line. Every request has a numeric `id` and an `op`. Every reply has the same `id` and `ok`, plus `error` when `ok` is false. The bridge serves one client, and the client applies the 2 s call timeout.

| Op | Request fields | Reply fields |
|---|---|---|
| `hello` | `protocol` | `protocol`, `lhm`, `controllable` (fan ids), `refused_chips` |
| `heartbeat` | | |
| `snapshot` | | `sensors`, `fans`, `metrics`, `degraded` (id → reason) |
| `set` | `fan`, `duty` (whole number, 0-100) | |
| `restore` | `fan` | |
| `shutdown` | | Hands every fan back, then exits. `ok` is false if any failed. |

- **`sensors`:** `id`, `label`, `kind` (`cpu` or `other`), `temp_celsius`.
- **`fans`:** `id`, `label`, `kind` (`fan` or `pump`), `rpm`, `duty_percent`, `controllable`. A reading LHM doesn't have is null, never 0.
- **`metrics`:** `id`, `kind` (`cpu_load` or `cpu_power`), `value`.

## Rules

- **Watchdog (S6):** any valid request counts as a heartbeat, and the client sends one at least every 1 s. After 5 s with none, every fan is handed back and the bridge exits.
- **Ends of session:** a disconnect, a bad session, `shutdown` or Ctrl-C all hand every fan back before exit.
- **Pump rule:** a header named pump is refused below 20%. LHM writes its stored value for about 100 ms on a change to software mode, and that value is 0 by default, so `SeededSettings` starts every control at 100. A first takeover of any header, or any retake of a pump, is refused if the stored value is under 20%.
- **Chips:** only NCT6687D (`/lpc/nct6687d/N`, name `Nuvoton NCT6687D`, chip id `0xD592`). Other chips are reported read-only and every write is refused, because the save fields mean something else on them. NCT6687D-R is refused too.
- **Taking over a header:** its mode and PWM command are read from the chip and saved to `bridge-originals.json` before the first write. If that can't be done, nothing is written. A header already manual with no saved record is refused and reported as degraded: it isn't ours.
- **Handing back:** the saved values are put into LHM's three private fields, then the public `SetDefault` writes them. The bridge never writes a register itself. The chip's `0xA00` flag is then read back. If it's still manual or can't be read, the bridge tries up to 3 times, then keeps the record and reports degraded.
- **Restart:** on start, the bridge hands back every header left in `bridge-originals.json`. A damaged or foreign file is left alone and all control is refused.
- **Writes (S8):** the duty is read back from the chip. LHM can drop a write silently if the ISA bus is busy, and it ignores a repeated value, so a retry nudges the duty by 1 first. A write that never lands is an error.

## Hardware checks before first use

These need Sam's review first. None has been run.

1. **Takeover and hand-back:** on a case fan or an empty header, `set` then `restore`: `0xA00` ends clear and the duty returns to the BIOS's.
2. **No low first write:** watch a header during its first `set`: it must not dip.
3. **Read-back timing:** the duty read-back allows 500 ms, which hasn't been measured.
4. **Kill test:** kill the bridge during a takeover, start it again: the header ends on the BIOS and the record is gone.
5. **Watchdog:** a client that stops sending: fans are handed back within 5 s.
6. **Report cost:** `GetReport` dumps 256 rows and runs on every takeover and hand-back, so time it against the 2 s call timeout.
7. **Through the daemon, as the service:** install the bridge beside the daemon under Program Files (`lhm-bridge\lhm-bridge.exe`), start the service, and check that `/fans` lists the headers and the pipe's server check passes. CI can't cover this, because the pipe only opens for SYSTEM.
8. **Anti-cheat mode:** with a fake `faceitclient.exe` running, fans go to the BIOS, the bridge stops and PawnIO shows `STOPPED`. After closing it and waiting 30 s, all three come back. Then repeat with a service restart in the middle.
9. **Real FACEIT:** check that the process names are right, and whether it starts with PawnIO disabled.
