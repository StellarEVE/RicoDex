use ricodex_core::{
    Asset, AssetKind, Command, Fingerprint, History, Item, ModelError, Project, Tick, Track,
    TrackKind, TICKS_PER_SECOND,
};

fn sample_asset(id: u64) -> Asset {
    Asset {
        id,
        fingerprint: Fingerprint::new(1_000_000 + id, 11, 22, 33),
        path_hint: format!("/media/clip{id}.mp4"),
        kind: AssetKind::Video,
        duration: Tick::from_seconds(60.0),
    }
}

/// A valid project: two assets, one video track, two non overlapping items.
fn sample_project() -> Project {
    let mut project = Project::new();
    project.assets.push(sample_asset(1));
    project.assets.push(sample_asset(2));
    project.add_track(Track::new(10, TrackKind::Video, "V1"));
    let items = project.items_mut(10).unwrap();
    items.push(Item::new(
        100,
        1,
        Tick::from_seconds(0.0),
        Tick::from_seconds(2.0),
        Tick::ZERO,
    ));
    items.push(Item::new(
        101,
        2,
        Tick::from_seconds(2.0),
        Tick::from_seconds(3.0),
        Tick::from_seconds(5.0),
    ));
    project
}

// ---------------------------------------------------------------- time

#[test]
fn tick_rate_divides_evenly_by_audio_sample_rates() {
    assert_eq!(TICKS_PER_SECOND % 44_100, 0);
    assert_eq!(TICKS_PER_SECOND % 48_000, 0);
}

#[test]
fn tick_sample_round_trip_at_44100_and_48000() {
    for rate in [44_100u32, 48_000] {
        for samples in [0i64, 1, 1_000, 44_100, 123_456, 4_800_000] {
            let tick = Tick::from_samples(samples, rate).expect("standard rates divide evenly");
            assert_eq!(tick.to_samples_floor(rate), samples, "rate {rate}");
        }
    }
}

#[test]
fn tick_frame_round_trip_at_2997_and_other_rates() {
    // 29.97 fps as the rational 30000/1001, plus common exact rates.
    for (num, den) in [(30_000u32, 1_001u32), (24, 1), (25, 1), (30, 1), (60, 1)] {
        let frame_len = Tick::frame_duration(num, den).expect("frame rate divides the tick rate");
        for frame in [0i64, 1, 30, 1_000, 86_400] {
            let tick = Tick::from_frame(frame, num, den).unwrap();
            assert_eq!(tick, frame_len.checked_mul(frame).unwrap());
        }
    }
    // 30,000 frames at 30000/1001 fps span exactly 1001 seconds.
    assert_eq!(
        Tick::from_frame(30_000, 30_000, 1_001).unwrap(),
        Tick(TICKS_PER_SECOND * 1_001)
    );
}

#[test]
fn tick_seconds_round_trip() {
    for seconds in [0.0, 0.5, 1.0, 3.25, 187.4] {
        let tick = Tick::from_seconds(seconds);
        assert!((tick.to_seconds() - seconds).abs() < 1e-9);
    }
}

#[test]
fn tick_checked_arithmetic() {
    let a = Tick(10);
    assert_eq!(a.checked_add(Tick(5)), Some(Tick(15)));
    assert_eq!(a.checked_sub(Tick(15)), Some(Tick(-5)));
    assert_eq!(Tick(i64::MAX).checked_add(Tick(1)), None);
    assert_eq!(Tick(i64::MAX).checked_mul(2), None);
}

// ---------------------------------------------------------- validation

#[test]
fn valid_project_passes_validation() {
    assert!(sample_project().validate().is_ok());
}

#[test]
fn duplicate_item_id_rejected() {
    let mut project = sample_project();
    project.items_mut(10).unwrap()[1].id = 100;
    assert_eq!(
        project.validate(),
        Err(ModelError::DuplicateItemId { id: 100 })
    );
}

#[test]
fn duplicate_item_id_across_tracks_rejected() {
    let mut project = sample_project();
    project.add_track(Track::new(11, TrackKind::Video, "V2"));
    project.items_mut(11).unwrap().push(Item::new(
        100,
        1,
        Tick::ZERO,
        Tick::from_seconds(1.0),
        Tick::ZERO,
    ));
    assert_eq!(
        project.validate(),
        Err(ModelError::DuplicateItemId { id: 100 })
    );
}

#[test]
fn overlapping_items_rejected() {
    let mut project = sample_project();
    project.items_mut(10).unwrap()[1].start = Tick::from_seconds(1.0);
    match project.validate() {
        Err(ModelError::ItemsOverlap { .. }) => {}
        other => panic!("expected overlap error, got {other:?}"),
    }
}

#[test]
fn unsorted_items_rejected() {
    let mut project = sample_project();
    project.items_mut(10).unwrap().swap(0, 1);
    match project.validate() {
        Err(ModelError::ItemsNotSorted { .. }) | Err(ModelError::ItemsOverlap { .. }) => {}
        other => panic!("expected ordering error, got {other:?}"),
    }
}

#[test]
fn zero_duration_rejected() {
    let mut project = sample_project();
    project.items_mut(10).unwrap()[0].duration = Tick::ZERO;
    assert_eq!(
        project.validate(),
        Err(ModelError::NonPositiveDuration { id: 100 })
    );
}

#[test]
fn unknown_asset_rejected() {
    let mut project = sample_project();
    project.items_mut(10).unwrap()[0].asset_id = 999;
    assert_eq!(
        project.validate(),
        Err(ModelError::UnknownAsset {
            item_id: 100,
            asset_id: 999
        })
    );
}

// ------------------------------------------------------------ commands

#[test]
fn insert_and_remove_item_commands() {
    let project = sample_project();
    let new_item = Item::new(
        102,
        1,
        Tick::from_seconds(5.0),
        Tick::from_seconds(1.5),
        Tick::ZERO,
    );
    let project = project
        .apply(&Command::InsertItem {
            track_id: 10,
            item: new_item,
        })
        .map_err(|(_, e)| e)
        .unwrap();
    assert_eq!(project.items(10).unwrap().len(), 3);

    let project = project
        .apply(&Command::RemoveItem { item_id: 102 })
        .map_err(|(_, e)| e)
        .unwrap();
    assert_eq!(project.items(10).unwrap().len(), 2);
}

#[test]
fn rejected_command_leaves_project_unchanged() {
    let project = sample_project();
    let before = project.clone();
    // Overlapping insert: starts inside item 100 and would collide.
    let clashing = Item::new(
        200,
        1,
        Tick::from_seconds(1.0),
        Tick::from_seconds(1.0),
        Tick::ZERO,
    );
    let result = project.apply(&Command::InsertItem {
        track_id: 10,
        item: clashing,
    });
    let (returned, error) = result.err().expect("command must be rejected");
    assert!(matches!(error, ricodex_core::CommandError::Invalid(_)));
    assert_eq!(returned, before, "the original project comes back untouched");
}

#[test]
fn move_and_trim_commands() {
    let project = sample_project();
    let project = project
        .apply(&Command::MoveItem {
            item_id: 101,
            to_track: None,
            new_start: Tick::from_seconds(6.0),
        })
        .map_err(|(_, e)| e)
        .unwrap();
    assert_eq!(project.item(101).unwrap().start, Tick::from_seconds(6.0));

    let project = project
        .apply(&Command::TrimItem {
            item_id: 101,
            new_start: None,
            new_source_in: Some(Tick::from_seconds(7.0)),
            new_duration: Some(Tick::from_seconds(2.0)),
        })
        .map_err(|(_, e)| e)
        .unwrap();
    let item = project.item(101).unwrap();
    assert_eq!(item.source_in, Tick::from_seconds(7.0));
    assert_eq!(item.duration, Tick::from_seconds(2.0));
}

#[test]
fn locked_track_rejects_item_edits() {
    let mut history = History::new(sample_project());
    history
        .apply(&Command::SetTrackLocked {
            track_id: 10,
            locked: true,
        })
        .unwrap();
    let result = history.apply(&Command::RemoveItem { item_id: 100 });
    assert!(matches!(
        result,
        Err(ricodex_core::CommandError::TrackLocked { track_id: 10 })
    ));
    assert_eq!(history.project().items(10).unwrap().len(), 2);
}

#[test]
fn track_flags_commands() {
    let project = sample_project()
        .apply(&Command::SetTrackMuted {
            track_id: 10,
            muted: true,
        })
        .map_err(|(_, e)| e)
        .unwrap();
    assert!(project.track(10).unwrap().muted);
}

// ------------------------------------------------------------- history

#[test]
fn undo_and_redo_restore_states() {
    let mut history = History::new(sample_project());
    let original = history.project().clone();

    history
        .apply(&Command::SetItemEnabled {
            item_id: 100,
            enabled: false,
        })
        .unwrap();
    assert!(!history.project().item(100).unwrap().enabled);
    assert!(history.can_undo());

    assert!(history.undo());
    assert_eq!(history.project(), &original);
    assert!(history.can_redo());

    assert!(history.redo());
    assert!(!history.project().item(100).unwrap().enabled);

    assert!(history.undo());
    assert!(!history.undo(), "second undo has nothing left");
    assert_eq!(history.project(), &original);
}

#[test]
fn rejected_command_does_not_touch_history() {
    let mut history = History::new(sample_project());
    let bad = Command::RemoveItem { item_id: 4242 };
    assert!(history.apply(&bad).is_err());
    assert!(!history.can_undo());
    assert!(!history.can_redo());
}

// --------------------------------------------------------- persistence

#[test]
fn save_load_round_trip_and_backup() {
    let dir = std::env::temp_dir().join(format!("ricodex-core-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("project.json");

    let project = sample_project();
    project.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded, project);
    assert!(
        !ricodex_core::backup_path(&path).exists(),
        "no backup after the first save"
    );

    // Second save, with a visible change, must produce a .bak of the first.
    let changed = project
        .clone()
        .apply(&Command::SetTrackMuted {
            track_id: 10,
            muted: true,
        })
        .map_err(|(_, e)| e)
        .unwrap();
    changed.save(&path).unwrap();

    let backup = Project::load(&ricodex_core::backup_path(&path)).unwrap();
    assert_eq!(backup, project, "backup holds the first save");
    let current = Project::load(&path).unwrap();
    assert_eq!(current, changed, "project file holds the second save");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn newer_schema_gets_a_clear_error() {
    let mut text = sample_project().to_json().unwrap();
    text = text.replace("\"schema_version\": 1", "\"schema_version\": 99");
    match Project::from_json(&text) {
        Err(ricodex_core::PersistenceError::NewerSchema { found: 99, .. }) => {}
        other => panic!("expected newer schema error, got {other:?}"),
    }
}

#[test]
fn settings_defaults_are_sane() {
    let project = Project::new();
    assert_eq!(project.export_settings.width, 1920);
    assert_eq!(project.generation_settings.continue_starts_bonus, 0.35);
    assert!(project.generation_settings.max_clip > project.generation_settings.min_clip);
}
