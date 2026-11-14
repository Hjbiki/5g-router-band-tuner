use router_core::event_log::{EventLog, EventRecord};
use router_core::journal::{Journal, JournalPhase, JournalRecord};
use router_core::{
    LteBandSet, NetConfig, NetworkMode, NrOption, SequenceOrigin, SequenceRecord, SequenceStore,
    SEQUENCE_VERSION,
};

fn record(id: &str, created_at_ms: u64, phase: JournalPhase) -> JournalRecord {
    JournalRecord {
        version: 1,
        id: id.to_string(),
        driver: "huawei".to_string(),
        host: "192.168.8.1".to_string(),
        previous: NetConfig {
            network_mode: NetworkMode::Auto,
            lte_bands: LteBandSet::AUTO,
            legacy_band_raw: "3FFFFFFF".to_string(),
            nr_option: Some(NrOption::Nsa),
        },
        requested: NetConfig {
            network_mode: NetworkMode::LteOnly,
            lte_bands: LteBandSet::from_bands(&[3]).unwrap(),
            legacy_band_raw: "3FFFFFFF".to_string(),
            nr_option: Some(NrOption::Nsa),
        },
        phase,
        phase_changed_at_ms: created_at_ms + 5,
        created_at_ms,
    }
}

#[test]
fn write_is_atomic_and_loadable() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::new(dir.path().to_path_buf());
    assert_eq!(journal.load().unwrap(), None);

    let r = record("a1b2", 1000, JournalPhase::Journaled);
    journal.write(&r).unwrap();
    assert_eq!(journal.load().unwrap(), Some(r.clone()));
    assert!(dir.path().join("pending-change.json").exists());
    assert!(!dir.path().join("pending-change.json.tmp").exists());

    let mut r2 = r.clone();
    r2.phase = JournalPhase::ReattachGrace;
    journal.write(&r2).unwrap();
    assert_eq!(journal.load().unwrap(), Some(r2));
    assert!(!dir.path().join("pending-change.json.tmp").exists());
}

#[test]
fn phase_serializes_snake_case() {
    let json = serde_json::to_string(&JournalPhase::ReattachGrace).unwrap();
    assert_eq!(json, "\"reattach_grace\"");
    let json = serde_json::to_string(&JournalPhase::RevertFailed).unwrap();
    assert_eq!(json, "\"revert_failed\"");
}

#[test]
fn corrupt_journal_is_an_error_not_none() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pending-change.json"), b"{not json").unwrap();
    let err = Journal::new(dir.path().to_path_buf()).load().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn archive_prunes_to_20() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::new(dir.path().to_path_buf());
    for i in 1..=25u64 {
        let r = record(&format!("id{i}"), i * 1000, JournalPhase::Committed);
        journal.write(&r).unwrap();
        journal.archive(&r).unwrap();
        assert_eq!(
            journal.load().unwrap(),
            None,
            "archive removes the pending journal"
        );
    }
    let archive = dir.path().join("journal-archive");
    let mut names: Vec<String> = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 20, "{names:?}");
    for i in 1..=5u64 {
        assert!(!names.contains(&format!("{}-id{i}.json", i * 1000)));
    }
    for i in 6..=25u64 {
        assert!(names.contains(&format!("{}-id{i}.json", i * 1000)));
    }
    let kept: JournalRecord =
        serde_json::from_slice(&std::fs::read(archive.join("25000-id25.json")).unwrap()).unwrap();
    assert_eq!(kept, record("id25", 25000, JournalPhase::Committed));
}

#[test]
fn archive_file_name_cannot_escape_the_archive_dir() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::new(dir.path().join("app"));
    let r = record("../../evil", 7, JournalPhase::Committed);
    journal.archive(&r).unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path().join("app").join("journal-archive"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(!names[0].contains(".."), "{names:?}");
    assert!(!dir.path().join("evil.json").exists());
}

#[test]
fn event_log_rotates() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::new(dir.path().to_path_buf());
    let filler = "x".repeat(1000);
    // ~4.5 MiB in total: enough to rotate more than three times.
    for i in 0..4700u64 {
        log.append(&EventRecord {
            at_ms: i,
            kind: "test".to_string(),
            detail: serde_json::json!({ "n": i, "filler": filler }),
        })
        .unwrap();
    }
    let p = |suffix: &str| dir.path().join(format!("events.jsonl{suffix}"));
    assert!(p("").exists());
    assert!(p(".1").exists());
    assert!(p(".2").exists());
    assert!(p(".3").exists());
    assert!(!p(".4").exists());
    for s in ["", ".1", ".2", ".3"] {
        let len = std::fs::metadata(p(s)).unwrap().len();
        assert!(len <= 1024 * 1024, "{s}: {len}");
    }
    // Every line is a complete JSON record.
    let text = std::fs::read_to_string(p(".1")).unwrap();
    for line in text.lines() {
        let r: EventRecord = serde_json::from_str(line).unwrap();
        assert_eq!(r.kind, "test");
    }
}

#[test]
fn journal_never_contains_secrets() {
    let r = record("a1b2", 1000, JournalPhase::Probation);
    let json = serde_json::to_string(&r).unwrap().to_lowercase();
    assert!(!json.contains("\"password\""), "{json}");
    assert!(!json.contains("\"token\""), "{json}");

    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::new(dir.path().to_path_buf());
    journal.write(&r).unwrap();
    let on_disk = std::fs::read_to_string(dir.path().join("pending-change.json"))
        .unwrap()
        .to_lowercase();
    assert!(!on_disk.contains("\"password\""));
    assert!(!on_disk.contains("\"token\""));
}

#[test]
fn archived_record_is_found_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::new(dir.path().to_path_buf());
    assert_eq!(journal.archived("a1b2").unwrap(), None, "no archive yet");
    let r = record("a1b2", 1000, JournalPhase::Committed);
    journal.write(&r).unwrap();
    journal.archive(&r).unwrap();
    let other = record("c3d4", 2000, JournalPhase::Reverted);
    journal.archive(&other).unwrap();
    assert_eq!(journal.archived("a1b2").unwrap(), Some(r));
    assert_eq!(journal.archived("c3d4").unwrap(), Some(other));
    assert_eq!(journal.archived("ffff").unwrap(), None);
}

fn sequence(step: u8) -> SequenceRecord {
    SequenceRecord {
        version: SEQUENCE_VERSION,
        driver: "huawei".into(),
        host: "192.168.8.1".into(),
        origin: SequenceOrigin::User,
        primary: 3,
        step,
        final_config: NetConfig {
            network_mode: NetworkMode::Auto,
            lte_bands: LteBandSet::from_bands(&[1, 3]).unwrap(),
            legacy_band_raw: "3FFFFFFF".into(),
            nr_option: Some(NrOption::Nsa),
        },
        original: None,
        change: Some("a1b2".into()),
        created_at_ms: 1000,
        step1: None,
    }
}

/// A band switch for a new IP: B3 alone for a moment, then B1+B3 again.
fn reattach_sequence(step: u8) -> SequenceRecord {
    let s = sequence(step);
    let step1 = NetConfig {
        lte_bands: LteBandSet::from_bands(&[3]).unwrap(),
        ..s.final_config.clone()
    };
    SequenceRecord {
        origin: SequenceOrigin::IpReattach,
        step1: Some(step1),
        ..s
    }
}

#[test]
fn sequence_file_round_trips_with_and_without_step1() {
    let dir = tempfile::tempdir().unwrap();
    let store = SequenceStore::new(dir.path().to_path_buf());
    let plain = sequence(1);
    store.write(&plain).unwrap();
    assert_eq!(store.load().unwrap(), Some(plain));
    let raw = std::fs::read_to_string(store.path()).unwrap();
    assert!(!raw.contains("step1"), "left out when there is none: {raw}");

    let reattach = reattach_sequence(1);
    store.write(&reattach).unwrap();
    assert_eq!(store.load().unwrap(), Some(reattach.clone()));
    let raw = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        raw.contains("\"step1\"") && raw.contains("\"ipReattach\""),
        "{raw}"
    );

    // A file from before step1 existed still loads.
    let old = raw.replace("\"origin\": \"ipReattach\"", "\"origin\": \"user\"");
    let old: serde_json::Value = serde_json::from_str(&old).unwrap();
    let mut old = old.as_object().unwrap().clone();
    old.remove("step1");
    std::fs::write(store.path(), serde_json::to_vec(&old).unwrap()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.step1, None);
    assert_eq!(loaded.origin, SequenceOrigin::User);
}

#[test]
fn reattach_sequence_shape_checks() {
    assert!(reattach_sequence(1).is_well_formed());
    assert!(reattach_sequence(2).is_well_formed());
    let mut no_step1 = reattach_sequence(1);
    no_step1.step1 = None;
    assert!(!no_step1.is_well_formed(), "needs its own step 1");
    let mut same = reattach_sequence(1);
    same.step1 = Some(same.final_config.clone());
    assert!(
        !same.is_well_formed(),
        "step 1 must differ from the final config"
    );
    let mut not_first = reattach_sequence(1);
    not_first.primary = 1;
    assert!(!not_first.is_well_formed(), "primary is a band of step 1");
    let mut user_with_step1 = reattach_sequence(1);
    user_with_step1.origin = SequenceOrigin::User;
    assert!(
        !user_with_step1.is_well_formed(),
        "only a band switch for a new IP carries step 1"
    );
}

#[test]
fn sequence_file_write_clear_and_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    let store = SequenceStore::new(dir.path().to_path_buf());
    assert_eq!(store.load().unwrap(), None);
    store.clear().unwrap();
    let s = sequence(1);
    store.write(&s).unwrap();
    assert_eq!(store.load().unwrap(), Some(s.clone()));
    assert!(!dir.path().join("pending-sequence.json.tmp").exists());
    store.write(&sequence(2)).unwrap();
    assert_eq!(store.load().unwrap().unwrap().step, 2);
    store.clear().unwrap();
    assert_eq!(store.load().unwrap(), None);

    std::fs::write(store.path(), "{ torn").unwrap();
    let err = store.load().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let aside = store.set_aside(42).unwrap();
    assert!(aside.ends_with("pending-sequence.ignored-42.json"));
    assert_eq!(std::fs::read_to_string(aside).unwrap(), "{ torn");
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn sequence_shape_checks() {
    assert!(sequence(1).is_well_formed());
    assert!(sequence(2).is_well_formed());
    assert!(!sequence(3).is_well_formed());
    let mut alone = sequence(1);
    alone.final_config.lte_bands = LteBandSet::from_bands(&[3]).unwrap();
    assert!(
        !alone.is_well_formed(),
        "the primary band alone is one step"
    );
    let mut missing = sequence(1);
    missing.primary = 20;
    assert!(
        !missing.is_well_formed(),
        "final set lacks the primary band"
    );
    let mut zero = sequence(1);
    zero.primary = 0;
    assert!(!zero.is_well_formed());
}
