//! HWiNFO64 shared-memory sensor source (read-only, optional). SPEC §10.
//!
//! The parser works on a byte copy of the shared memory, so it is safe code and runs on every OS.
//! Only `hwinfo_win` (Windows) touches the real mapping.

use crate::{Capabilities, HalError, HardwareLayer};
use std::fmt;
use thermaliq_common::{HardwareSnapshot, SensorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwinfoError {
    NotRunning,
    PermissionDenied,
    Os(u32),
    SignatureMismatch { found: u32 },
    UnsupportedVersion { version: u32 },
    UnsupportedRevision { revision: u32 },
    LayoutInvalid { field: &'static str },
    Stale,
}

impl fmt::Display for HwinfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str(
                "HWiNFO64 shared memory not found: run HWiNFO64 with Shared Memory Support on \
                 (https://www.hwinfo.com/download/)",
            ),
            Self::PermissionDenied => f.write_str(
                "HWiNFO64 shared memory: access denied (run the daemon as the same admin level)",
            ),
            Self::Os(code) => write!(f, "HWiNFO64 shared memory: Windows error {code}"),
            Self::SignatureMismatch { found } => {
                write!(f, "HWiNFO64 header signature {found:08X}, expected 'HWiS'")
            }
            Self::UnsupportedVersion { version } => write!(
                f,
                "HWiNFO64 layout version {version} not supported ({MIN_VERSION}-{MAX_VERSION})"
            ),
            Self::UnsupportedRevision { revision } => write!(
                f,
                "HWiNFO64 layout revision {revision} not supported (0-{MAX_REVISION})"
            ),
            Self::LayoutInvalid { field } => write!(f, "HWiNFO64 layout invalid: {field}"),
            Self::Stale => f.write_str(
                "HWiNFO64 stopped updating its shared memory (the free version stops after \
                 12 hours: restart HWiNFO64)",
            ),
        }
    }
}

// Layout from the HWiNFO SDK (SENS_SM2): packed, little endian.
const SIGNATURE: u32 = u32::from_le_bytes(*b"HWiS");
const MIN_VERSION: u32 = 1;
const MAX_VERSION: u32 = 3;
const MAX_REVISION: u32 = 9;
const MAX_COUNT: u32 = 8192;
const MAX_ELEMENT: u32 = 4096;
const SENSOR: usize = 264;
const READING: usize = 316;
const READING_TYPE_TEMP: u32 = 1;
/// HWiNFO polls every 2 s by default; a poll time this old means it has stopped.
const STALE_SECS: i64 = 30;

impl std::error::Error for HwinfoError {}

impl From<HwinfoError> for HalError {
    fn from(e: HwinfoError) -> Self {
        HalError(e.to_string())
    }
}

type Reader = Box<dyn Fn() -> Result<Vec<u8>, HwinfoError> + Send + Sync>;

/// Read-only: temperatures only, no fans, nothing to write.
pub struct Hwinfo {
    read: Reader,
}

impl Hwinfo {
    /// Opens the real shared memory on every snapshot, so a restarted HWiNFO is picked up.
    #[cfg(windows)]
    pub fn new() -> Self {
        Self::with_reader(crate::hwinfo_win::read)
    }

    /// `read` returns a copy of the shared-memory block.
    pub fn with_reader(
        read: impl Fn() -> Result<Vec<u8>, HwinfoError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            read: Box::new(read),
        }
    }
}

#[cfg(windows)]
impl Default for Hwinfo {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareLayer for Hwinfo {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        Ok(parse(&(self.read)()?, unix_now())?)
    }

    fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), HalError> {
        Err(HalError("HWiNFO is read-only".into()))
    }

    fn restore_auto(&self, _: &str) -> Result<(), HalError> {
        Err(HalError("HWiNFO is read-only".into()))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

/// Classifies by HWiNFO's sensor (group) name, never by a reading's label: the board chip
/// has a reading called "CPU" that belongs to no CPU group.
pub(crate) fn kind_of(sensor_name: &str) -> SensorKind {
    if sensor_name.starts_with("CPU [#") {
        SensorKind::Cpu
    } else if sensor_name.starts_with("GPU [#") {
        SensorKind::Gpu
    } else {
        SensorKind::Other
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The `count` elements of `size` bytes at `off`, or None if they don't fit in `buf`.
fn section(buf: &[u8], off: u32, size: u32, count: u32) -> Option<impl Iterator<Item = &[u8]>> {
    let (off, size) = (off as usize, size as usize);
    let end = off.checked_add(size.checked_mul(count as usize)?)?;
    Some(buf.get(off..end)?.chunks_exact(size))
}

/// Header checks, stale check, then every temperature reading. `now` is Unix seconds.
pub(crate) fn parse(buf: &[u8], now: i64) -> Result<HardwareSnapshot, HwinfoError> {
    let bad = |field| HwinfoError::LayoutInvalid { field };
    let word = |off| u32_at(buf, off).ok_or(bad("header"));
    if word(0)? != SIGNATURE {
        return Err(HwinfoError::SignatureMismatch { found: word(0)? });
    }
    let (version, revision) = (word(4)?, word(8)?);
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(HwinfoError::UnsupportedVersion { version });
    }
    if revision > MAX_REVISION {
        return Err(HwinfoError::UnsupportedRevision { revision });
    }
    let poll_time = buf
        .get(12..20)
        .and_then(|b| b.try_into().ok())
        .map(i64::from_le_bytes)
        .ok_or(bad("header"))?;
    let (s_off, s_size, s_n) = (word(20)?, word(24)?, word(28)?);
    let (r_off, r_size, r_n) = (word(32)?, word(36)?, word(40)?);
    if !(SENSOR as u32..=MAX_ELEMENT).contains(&s_size) {
        return Err(bad("sensor_element_size"));
    }
    if !(READING as u32..=MAX_ELEMENT).contains(&r_size) {
        return Err(bad("reading_element_size"));
    }
    if s_n > MAX_COUNT {
        return Err(bad("sensor_element_count"));
    }
    if r_n > MAX_COUNT {
        return Err(bad("reading_element_count"));
    }
    let sensors: Vec<&[u8]> = section(buf, s_off, s_size, s_n)
        .ok_or(bad("sensor section"))?
        .collect();
    let readings = section(buf, r_off, r_size, r_n).ok_or(bad("reading section"))?;
    // After the layout checks so a garbage header reports as garbage, not as stale.
    if poll_time == 0 || now - poll_time > STALE_SECS {
        return Err(HwinfoError::Stale);
    }

    let mut snap = HardwareSnapshot::default();
    for r in readings {
        let Some(group) = u32_at(r, 4).and_then(|i| sensors.get(i as usize)) else {
            continue;
        };
        let value = f64::from_le_bytes(r[284..292].try_into().unwrap_or_default());
        let label = match text(&r[140..268]) {
            l if l.trim().is_empty() => text(&r[12..140]),
            l => l,
        };
        // "Distance to TjMAX" is headroom, not a temperature: a low value would hide a hot core.
        if u32_at(r, 0) != Some(READING_TYPE_TEMP)
            || !(value > -40.0 && value < 150.0)
            || label.contains("Distance to")
        {
            continue;
        }
        let (Some(sensor_id), Some(inst), Some(reading_id)) =
            (u32_at(group, 0), u32_at(group, 4), u32_at(r, 8))
        else {
            continue;
        };
        let name = text(&group[8..136]);
        snap.sensors.push(thermaliq_common::SensorReading {
            id: format!("hwinfo_{sensor_id:08x}_{inst}_{reading_id:08x}"),
            label: format!("{label} ({name})"),
            kind: kind_of(&name),
            temp_celsius: value as f32,
        });
    }
    Ok(snap)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SIG: u32 = u32::from_le_bytes(*b"HWiS");
    const HEADER: usize = 44;
    const SENSOR: usize = 264;
    const READING: usize = 316;
    const TEMP: u32 = 1;
    const FAN: u32 = 3;

    /// (sensor_id, inst, group name, readings as (type, label, value)).
    type Group = (u32, u32, &'static str, Vec<(u32, &'static str, f64)>);

    fn put(buf: &mut [u8], off: usize, s: &str) {
        buf[off..off + s.len()].copy_from_slice(s.as_bytes());
    }

    /// A buffer laid out like HWiNFO's SENS_SM2 (packed, little endian).
    fn build(poll_time: i64, groups: &[Group]) -> Vec<u8> {
        let readings: usize = groups.iter().map(|g| g.3.len()).sum();
        let sensor_off = HEADER;
        let reading_off = sensor_off + groups.len() * SENSOR;
        let mut b = vec![0u8; reading_off + readings * READING];
        let h = [
            SIG,
            2,
            0,
            poll_time as u32,
            (poll_time >> 32) as u32,
            sensor_off as u32,
            SENSOR as u32,
            groups.len() as u32,
            reading_off as u32,
            READING as u32,
            readings as u32,
        ];
        for (i, v) in h.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut r = reading_off;
        for (gi, (id, inst, name, rs)) in groups.iter().enumerate() {
            let s = sensor_off + gi * SENSOR;
            b[s..s + 4].copy_from_slice(&id.to_le_bytes());
            b[s + 4..s + 8].copy_from_slice(&inst.to_le_bytes());
            put(&mut b, s + 8, name);
            put(&mut b, s + 136, name);
            for (ri, (ty, label, v)) in rs.iter().enumerate() {
                b[r..r + 4].copy_from_slice(&ty.to_le_bytes());
                b[r + 4..r + 8].copy_from_slice(&(gi as u32).to_le_bytes());
                b[r + 8..r + 12].copy_from_slice(&(0x100 + ri as u32).to_le_bytes());
                put(&mut b, r + 12, label);
                put(&mut b, r + 268, "C");
                b[r + 284..r + 292].copy_from_slice(&v.to_le_bytes());
                r += READING;
            }
        }
        b
    }

    fn machine() -> Vec<Group> {
        vec![
            (
                0xF000_0000,
                0,
                "CPU [#0]: Intel Core i7-12700K: DTS",
                vec![
                    (TEMP, "Core 0", 45.0),
                    (TEMP, "CPU Package", 61.0),
                    (TEMP, "Core 0 Distance to TjMAX", 55.0),
                ],
            ),
            (
                0xE000_0000,
                0,
                "GPU [#0]: NVIDIA GeForce RTX 3080:",
                vec![
                    (TEMP, "GPU Temperature", 52.0),
                    (TEMP, "GPU Hot Spot Temperature", 63.0),
                    (FAN, "GPU Fan", 1200.0),
                ],
            ),
            (
                0x8000_0000,
                0,
                "ASUS ROG STRIX Z690-E GAMING WIFI (Nuvoton NCT6798D)",
                vec![(TEMP, "CPU", 38.0), (FAN, "CPU Fan", 900.0)],
            ),
        ]
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn signature_is_hwis() {
        assert_eq!(SIG.to_le_bytes(), *b"HWiS");
    }

    #[test]
    fn parses_temperatures_and_ignores_fans_and_distance_to_tjmax() {
        let snap = parse(&build(NOW, &machine()), NOW).unwrap();
        let ids: Vec<_> = snap.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hwinfo_f0000000_0_00000100",
                "hwinfo_f0000000_0_00000101",
                "hwinfo_e0000000_0_00000100",
                "hwinfo_e0000000_0_00000101",
                "hwinfo_80000000_0_00000100",
            ]
        );
        assert!(snap.fans.is_empty());
        assert_eq!(snap.sensors[1].temp_celsius, 61.0);
        assert_eq!(
            snap.sensors[1].label,
            "CPU Package (CPU [#0]: Intel Core i7-12700K: DTS)"
        );
    }

    #[test]
    fn kinds_come_from_the_group_not_the_label() {
        let snap = parse(&build(NOW, &machine()), NOW).unwrap();
        let kinds: Vec<_> = snap.sensors.iter().map(|s| s.kind).collect();
        use SensorKind::*;
        // The board chip's reading is labelled "CPU" but belongs to a motherboard group.
        assert_eq!(kinds, [Cpu, Cpu, Gpu, Gpu, Other]);
    }

    #[test]
    fn kind_of_real_hwinfo_group_names() {
        use SensorKind::*;
        for (name, want) in [
            ("CPU [#0]: AMD Ryzen 7 5800X3D: Enhanced", Cpu),
            ("CPU [#0]: Intel Core i7-12700K: DTS", Cpu),
            ("CPU [#1]: Intel Xeon Gold 6130", Cpu),
            ("GPU [#0]: NVIDIA GeForce RTX 3080:", Gpu),
            ("GPU [#1]: AMD Radeon RX 6800 XT:", Gpu),
            ("ASUS ROG STRIX X570-E GAMING (Nuvoton NCT6798D)", Other),
            ("Memory Timings", Other),
            ("S.M.A.R.T.: Samsung SSD 980 PRO 1TB (S5GXNF0R)", Other),
            ("Drive: Samsung SSD 980 PRO 1TB", Other),
            ("Network: Intel Ethernet Controller I225-V", Other),
            ("", Other),
        ] {
            assert_eq!(kind_of(name), want, "{name}");
        }
    }

    #[test]
    fn user_renamed_group_still_classifies_by_the_original_name() {
        let mut b = build(NOW, &machine());
        b[HEADER + 136..HEADER + 264].fill(0);
        put(&mut b, HEADER + 136, "My renamed box");
        let snap = parse(&b, NOW).unwrap();
        assert_eq!(snap.sensors[0].kind, SensorKind::Cpu);
    }

    #[test]
    fn user_label_wins_over_original_label() {
        let mut b = build(NOW, &machine());
        let r = HEADER + 3 * SENSOR + 140;
        put(&mut b, r, "Package");
        let snap = parse(&b, NOW).unwrap();
        assert!(snap.sensors[0].label.starts_with("Package ("));
    }

    #[test]
    fn implausible_temperatures_are_skipped() {
        let g: Vec<Group> = vec![(
            1,
            0,
            "CPU [#0]: x",
            vec![
                (TEMP, "a", 150.0),
                (TEMP, "b", -40.0),
                (TEMP, "c", f64::NAN),
                (TEMP, "d", 70.0),
            ],
        )];
        let snap = parse(&build(NOW, &g), NOW).unwrap();
        assert_eq!(snap.sensors.len(), 1);
        assert_eq!(snap.sensors[0].temp_celsius, 70.0);
    }

    #[test]
    fn a_reading_pointing_past_the_last_sensor_is_skipped() {
        let mut b = build(NOW, &machine());
        let r = HEADER + 3 * SENSOR;
        b[r + 4..r + 8].copy_from_slice(&99u32.to_le_bytes());
        let snap = parse(&b, NOW).unwrap();
        assert_eq!(snap.sensors.len(), 4);
    }

    #[test]
    fn ids_survive_a_reorder_of_the_groups() {
        let mut m = machine();
        let a = parse(&build(NOW, &m), NOW).unwrap();
        m.reverse();
        let b = parse(&build(NOW, &m), NOW).unwrap();
        let find = |s: &HardwareSnapshot| {
            s.sensors
                .iter()
                .find(|r| r.label.starts_with("CPU Package"))
                .unwrap()
                .id
                .clone()
        };
        assert_eq!(find(&a), find(&b));
    }

    #[test]
    fn bad_signature() {
        let mut b = build(NOW, &machine());
        b[0] = 0;
        assert!(matches!(
            parse(&b, NOW),
            Err(HwinfoError::SignatureMismatch { .. })
        ));
    }

    #[test]
    fn version_and_revision_ranges() {
        for (v, ok) in [(0u32, false), (1, true), (3, true), (4, false), (99, false)] {
            let mut b = build(NOW, &machine());
            b[4..8].copy_from_slice(&v.to_le_bytes());
            assert_eq!(parse(&b, NOW).is_ok(), ok, "version {v}");
        }
        let mut b = build(NOW, &machine());
        b[8..12].copy_from_slice(&10u32.to_le_bytes());
        assert!(matches!(
            parse(&b, NOW),
            Err(HwinfoError::UnsupportedRevision { revision: 10 })
        ));
    }

    #[test]
    fn element_sizes_must_hold_the_struct_we_read() {
        for off in [24usize, 36] {
            for bad in [0u32, 4, 263, 5000] {
                let mut b = build(NOW, &machine());
                b[off..off + 4].copy_from_slice(&bad.to_le_bytes());
                assert!(
                    matches!(parse(&b, NOW), Err(HwinfoError::LayoutInvalid { .. })),
                    "offset {off} size {bad}"
                );
            }
        }
    }

    #[test]
    fn huge_counts_are_refused() {
        for off in [28usize, 40] {
            let mut b = build(NOW, &machine());
            b[off..off + 4].copy_from_slice(&99_999u32.to_le_bytes());
            assert!(matches!(
                parse(&b, NOW),
                Err(HwinfoError::LayoutInvalid { .. })
            ));
        }
    }

    #[test]
    fn sections_running_past_the_buffer_are_refused_not_read() {
        let mut b = build(NOW, &machine());
        b.truncate(b.len() - 1);
        assert!(matches!(
            parse(&b, NOW),
            Err(HwinfoError::LayoutInvalid { .. })
        ));
        let mut b = build(NOW, &machine());
        b[32..36].copy_from_slice(&u32::MAX.to_le_bytes()); // reading offset
        assert!(matches!(
            parse(&b, NOW),
            Err(HwinfoError::LayoutInvalid { .. })
        ));
    }

    #[test]
    fn a_buffer_shorter_than_the_header_is_refused() {
        assert!(parse(&[], NOW).is_err());
        assert!(parse(&[0u8; 43], NOW).is_err());
    }

    #[test]
    fn stale_when_never_polled_or_not_updated_recently() {
        assert_eq!(parse(&build(0, &machine()), NOW), Err(HwinfoError::Stale));
        assert_eq!(
            parse(&build(NOW - 31, &machine()), NOW),
            Err(HwinfoError::Stale)
        );
        assert!(parse(&build(NOW - 29, &machine()), NOW).is_ok());
        // A clock slightly behind HWiNFO's is not stale.
        assert!(parse(&build(NOW + 5, &machine()), NOW).is_ok());
    }

    #[test]
    fn layer_reads_through_the_reader_and_is_read_only() {
        let h = Hwinfo::with_reader(|| Ok(build(unix_now(), &machine())));
        let snap = h.snapshot().unwrap();
        assert_eq!(snap.sensors.len(), 5);
        assert!(h.capabilities().controllable_fans.is_empty());
        assert!(h.set_fan_duty("hwinfo_x", 50).is_err());
        assert!(h.restore_auto("hwinfo_x").is_err());
    }

    #[test]
    fn layer_errors_when_hwinfo_is_not_running() {
        let h = Hwinfo::with_reader(|| Err(HwinfoError::NotRunning));
        assert!(h.snapshot().is_err());
    }

    #[test]
    fn layer_errors_on_stale_memory() {
        let h = Hwinfo::with_reader(|| Ok(build(unix_now() - 3600, &machine())));
        assert!(h.snapshot().is_err());
    }

    #[test]
    fn errors_name_the_problem() {
        assert!(HwinfoError::NotRunning.to_string().contains("hwinfo.com"));
        assert!(HwinfoError::SignatureMismatch { found: 0xDEAD_BEEF }
            .to_string()
            .contains("DEADBEEF"));
        assert!(HwinfoError::Stale.to_string().contains("12"));
    }

    /// Needs HWiNFO64 running with "Shared Memory Support" on. Run elevated if HWiNFO is.
    /// `cargo test -p thermaliq-hal -- --ignored --nocapture hwinfo`
    #[test]
    #[ignore = "needs HWiNFO64 running on Windows"]
    #[cfg(windows)]
    fn hardware_hwinfo_prints_cpu_and_gpu_sensors() {
        let snap = Hwinfo::new().snapshot().unwrap();
        for s in &snap.sensors {
            println!(
                "{:?}\t{:>6.1} C\t{}\t{}",
                s.kind, s.temp_celsius, s.id, s.label
            );
        }
        assert!(
            snap.sensors.iter().any(|s| s.kind == SensorKind::Cpu),
            "no CPU sensor"
        );
        assert!(
            snap.sensors.iter().any(|s| s.kind == SensorKind::Gpu),
            "no GPU sensor"
        );
    }
}
