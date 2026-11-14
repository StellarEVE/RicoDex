use ricodex_core::{
    Asset, AssetKind, ClipTransition, Command, Fingerprint, History, Item, ItemProvenance,
    ModelError, Project, Tick, Track, TrackKind, TransitionKind, TransitionParams, TICKS_PER_SECOND,
};

fn sample_asset(id: u64) -> Asset {
    Asset {
        id,
        fingerprint: Fingerprint::new(1_000_000 + id, 11, 22, 33),
        path_hint: format!("/media/clip{id}.mp4"),
        kind: AssetKind::Video,
        duration: Tick::from_seconds(60.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
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
fn item_source_range_must_respect_asset_trims() {
    let mut project = sample_project();
    let asset = project.assets.iter_mut().find(|asset| asset.id == 1).unwrap();
    asset.intro_trim = Tick::from_seconds(2.0);
    asset.outro_trim = Tick::from_seconds(3.0);
    assert_eq!(
        project.validate(),
        Err(ModelError::ItemOutsideAssetTrimWindow {
            item_id: 100,
            asset_id: 1,
        })
    );

    project.items_mut(10).unwrap()[0].source_in = Tick::from_seconds(2.0);
    assert!(project.validate().is_ok());

    project.items_mut(10).unwrap()[0].source_in = Tick::from_seconds(56.0);
    assert_eq!(
        project.validate(),
        Err(ModelError::ItemOutsideAssetTrimWindow {
            item_id: 100,
            asset_id: 1,
        })
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
fn schema_one_projects_migrate_source_metadata_and_single_song() {
    let mut project = sample_project();
    let mut song = sample_asset(3);
    song.kind = AssetKind::Audio;
    song.path_hint = "/media/song.wav".to_string();
    project.assets.push(song);

    let json = project.to_json().unwrap();
    let mut legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
    legacy["schema_version"] = serde_json::Value::from(1);
    let object = legacy.as_object_mut().unwrap();
    object.remove("song_asset_id");
    for asset in object
        .get_mut("assets")
        .unwrap()
        .as_array_mut()
        .unwrap()
    {
        let asset = asset.as_object_mut().unwrap();
        asset.remove("intro_trim");
        asset.remove("outro_trim");
    }

    let migrated = Project::from_json(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.schema_version, 4);
    assert_eq!(migrated.song_asset_id, Some(3));
    assert!(migrated.track_items.iter().flat_map(|group| &group.items).all(|item| {
        item.provenance == ItemProvenance::Manual && item.pinned
    }));
    assert!(migrated.assets.iter().all(|asset| {
        asset.intro_trim == Tick::ZERO && asset.outro_trim == Tick::ZERO
    }));
}


#[test]
fn schema_two_projects_migrate_clips_as_manual_and_pinned() {
    let project = sample_project();
    let mut legacy: serde_json::Value = serde_json::from_str(&project.to_json().unwrap()).unwrap();
    legacy["schema_version"] = serde_json::Value::from(2);
    for group in legacy["track_items"].as_array_mut().unwrap() {
        for item in group["items"].as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("provenance");
            item.as_object_mut().unwrap().remove("pinned");
        }
    }

    let migrated = Project::from_json(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.schema_version, 4);
    assert!(migrated.track_items.iter().flat_map(|group| &group.items).all(|item| {
        item.provenance == ItemProvenance::Manual && item.pinned
    }));
}



#[test]
fn schema_three_projects_migrate_without_losing_provenance_or_pins() {
    let project = sample_project()
        .apply(&Command::MarkItemManual { item_id: 100 })
        .map_err(|(_, error)| error)
        .unwrap();
    let mut legacy: serde_json::Value = serde_json::from_str(&project.to_json().unwrap()).unwrap();
    legacy["schema_version"] = serde_json::Value::from(3);
    for group in legacy["track_items"].as_array_mut().unwrap() {
        for item in group["items"].as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("transition_in");
        }
    }
    let migrated = Project::from_json(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.schema_version, 4);
    assert_eq!(migrated.item(100).unwrap().provenance, ItemProvenance::Manual);
    assert!(migrated.item(100).unwrap().pinned);
    assert!(migrated.item(101).unwrap().transition_in.is_none());
}

#[test]
fn item_transition_round_trips_and_is_undoable() {
    let transition = ClipTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(0.5),
    };
    let mut history = History::new(sample_project());
    history.apply(&Command::SetItemTransition {
        item_id: 101,
        transition: Some(transition),
    }).unwrap();
    assert_eq!(history.project().item(101).unwrap().transition_in, Some(transition));

    let saved = history.project().to_json().unwrap();
    let loaded = Project::from_json(&saved).unwrap();
    assert_eq!(loaded.item(101).unwrap().transition_in, Some(transition));

    assert!(history.undo());
    assert!(history.project().item(101).unwrap().transition_in.is_none());
    assert!(history.redo());
    assert_eq!(history.project().item(101).unwrap().transition_in, Some(transition));
}

#[test]
fn transition_requires_adjacent_video_items_and_valid_duration() {
    let transition = ClipTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(0.5),
    };
    let mut project = sample_project();
    project.items_mut(10).unwrap()[1].transition_in = Some(transition);
    assert!(project.validate().is_ok());

    let mut not_adjacent = project.clone();
    not_adjacent.items_mut(10).unwrap()[1].start = Tick::from_seconds(2.25);
    assert!(matches!(
        not_adjacent.validate(),
        Err(ModelError::TransitionItemsNotAdjacent { item_id: 101, .. })
    ));

    let mut too_long = project.clone();
    too_long.items_mut(10).unwrap()[1].transition_in = Some(ClipTransition {
        duration: Tick::from_seconds(2.0),
        ..transition
    });
    assert_eq!(
        too_long.validate(),
        Err(ModelError::InvalidTransitionDuration { item_id: 101 })
    );

    let mut first_item = project.clone();
    first_item.items_mut(10).unwrap()[0].transition_in = Some(transition);
    assert_eq!(
        first_item.validate(),
        Err(ModelError::TransitionWithoutPreviousItem { item_id: 100 })
    );
}

#[test]
fn transitions_at_both_ends_cannot_consume_an_entire_clip() {
    let mut project = sample_project();
    project.items_mut(10).unwrap()[0].duration = Tick::from_seconds(2.0);
    project.items_mut(10).unwrap()[1].start = Tick::from_seconds(2.0);
    project.items_mut(10).unwrap()[1].duration = Tick::from_seconds(3.0);
    project.items_mut(10).unwrap()[1].transition_in = Some(ClipTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(1.5),
    });
    project.items_mut(10).unwrap().push(Item::new(
        102, 1, Tick::from_seconds(5.0), Tick::from_seconds(2.0), Tick::from_seconds(10.0),
    ));
    project.items_mut(10).unwrap()[2].transition_in = Some(ClipTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(1.5),
    });
    assert_eq!(
        project.validate(),
        Err(ModelError::TransitionRegionsConsumeItem { item_id: 101 })
    );
}

#[test]
fn user_item_edits_mark_the_clip_manual_and_pinned() {
    let project = sample_project();
    let trimmed = project.apply(&Command::TrimItem {
        item_id: 100,
        new_start: None,
        new_source_in: Some(Tick::from_seconds(1.0)),
        new_duration: Some(Tick::from_seconds(1.5)),
    }).map_err(|(_, error)| error).unwrap();
    let item = trimmed.item(100).unwrap();
    assert_eq!(item.provenance, ItemProvenance::Manual);
    assert!(item.pinned);

    let unpinned = trimmed.apply(&Command::SetItemPinned {
        item_id: 100,
        pinned: false,
    }).map_err(|(_, error)| error).unwrap();
    assert!(!unpinned.item(100).unwrap().pinned);
    assert_eq!(unpinned.item(100).unwrap().provenance, ItemProvenance::Manual);
}

#[test]
fn song_selection_and_source_trims_round_trip() {
    let mut project = sample_project();
    let mut song = sample_asset(3);
    song.kind = AssetKind::Audio;
    project.assets.push(song);
    project.song_asset_id = Some(3);
    project.assets[0].intro_trim = Tick::from_seconds(1.25);
    project.assets[0].outro_trim = Tick::from_seconds(2.0);
    project.items_mut(10).unwrap()[0].source_in = Tick::from_seconds(1.25);
    let loaded = Project::from_json(&project.to_json().unwrap()).unwrap();
    assert_eq!(loaded.song_asset_id, Some(3));
    assert_eq!(loaded.assets[0].intro_trim, Tick::from_seconds(1.25));
    assert_eq!(loaded.assets[0].outro_trim, Tick::from_seconds(2.0));
}

#[test]
fn newer_schema_gets_a_clear_error() {
    let mut text = sample_project().to_json().unwrap();
    text = text.replace("\"schema_version\": 4", "\"schema_version\": 99");
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
