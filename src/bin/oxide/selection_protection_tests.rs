mod selection_protection {
    use super::*;

    fn fixture() -> (Fixture, Store, PathBuf, Vec<Row>) {
        let fixture = Fixture::new();
        let checkout = fixture.root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let outer = fixture.root.join("oxide-outputs");
        fs::create_dir(&outer).unwrap();
        let control = fixture.root.join("control");
        let mut rows = Vec::new();
        for member in ["debug", "release"] {
            let mut row = row_for(&fixture.root);
            row.checkout = checkout.clone();
            row.checkout_volume_uuid = checkout_volume_uuid(&checkout);
            row.member = member.to_owned();
            row.root = outer.join(member);
            row.receipt = outputs::receipt_path_public(&control, &row.checkout_id);
            fs::create_dir(&row.root).unwrap();
            write_root_marker(&row.root, &root_marker(&row));
            let observed = observe_fixture_root(&row);
            row.root_dev = Some(observed.dev);
            row.root_ino = Some(observed.ino);
            row.marker_digest = Some(observed.marker_digest);
            rows.push(row);
        }
        let control = ready_control(&fixture, rows.clone(), Vec::new());
        let store = Store::for_tests(&fixture.root.join("pool"));
        (fixture, store, control, rows)
    }

    fn receipt(rows: &[Row]) -> serde_json::Value {
        let selection = |row: &Row| {
            serde_json::json!({
                "kind": "external", "root": row.root, "volume_uuid": row.volume_uuid,
                "nonce": row.nonce, "generation": row.generation,
            })
        };
        serde_json::json!({
            "version": "oxide.outputs.transition/3", "layout": "profile",
            "checkout": rows[0].checkout, "checkout_id": rows[0].checkout_id,
            "generation": 1, "state": "committed", "updated_ms": 1,
            "roots": rows.iter().map(selection).collect::<Vec<_>>(),
            "links": rows.iter().map(|row| serde_json::json!({
                "member": row.member, "selected": selection(row),
            })).collect::<Vec<_>>(),
        })
    }

    fn evaluate_fixture(store: &Store, control: &Path) -> Evaluation {
        let registry = ready_registry(control).unwrap();
        evaluate_test(store, control, &registry, None).unwrap()
    }

    #[test]
    fn selected_present_and_nonrepresentative_profile_are_kept() {
        let (_fixture, store, control, rows) = fixture();
        let document = receipt(&rows);
        publish_snapshot_receipt(&control, &rows[0], &document);
        let parsed =
            outputs::read_receipt_for_prune(&control, &rows[0].checkout, &rows[0].checkout_id)
                .unwrap()
                .unwrap();
        assert_ne!(parsed.current().unwrap().root(), rows[1].root);
        assert!(current_selection_references(&parsed, &rows[1].root));
        let result = evaluate_fixture(&store, &control);
        assert!(result.complete);
        assert_eq!(result.len(), 2);
        for entry in result.iter() {
            assert!(entry.candidate.is_none());
            assert_eq!(entry.kept.as_deref(), Some(CURRENT_SELECTION_KEEP));
        }
    }

    #[test]
    fn contradictory_selected_references_still_refuse_the_whole_batch() {
        for fault in [
            "registry flag",
            "receipt selection",
            "nonce",
            "volume",
            "malformed",
        ] {
            let (_fixture, store, control, mut rows) = fixture();
            let mut document = receipt(&rows);
            match fault {
                "registry flag" => rows[0].selected = false,
                "receipt selection" => document["links"][0]["selected"] = serde_json::Value::Null,
                "nonce" => {
                    document["links"][0]["selected"]["nonce"] = serde_json::json!("e".repeat(32))
                }
                "volume" => {
                    document["links"][0]["selected"]["volume_uuid"] =
                        serde_json::json!("99999999-8888-7777-6666-555555555555")
                }
                "malformed" => {}
                _ => unreachable!(),
            }
            write_snapshot_registry(&control, rows.clone(), Vec::new());
            publish_snapshot_receipt(&control, &rows[0], &document);
            if fault == "malformed" {
                fs::write(&rows[0].receipt, b"{").unwrap();
            }
            let result = evaluate_fixture(&store, &control);
            assert!(!result.complete, "{fault}");
            assert_eq!(result.len(), 2);
            for entry in result.iter() {
                assert!(entry.candidate.is_none(), "{fault}");
                assert!(
                    entry.kept.as_deref().unwrap().contains("whole batch"),
                    "{fault}"
                );
            }
        }
    }

    #[test]
    fn selected_sibling_identity_disagreement_remains_incomplete() {
        let (_fixture, store, control, mut rows) = fixture();
        rows[1].root = rows[0].root.clone();
        rows[1].root_ino = rows[0].root_ino;
        rows[1].marker_digest = Some("e".repeat(64));
        write_snapshot_registry(&control, rows.clone(), Vec::new());
        let mut document = receipt(&rows);
        document["roots"] = serde_json::json!([document["roots"][0].clone()]);
        publish_snapshot_receipt(&control, &rows[0], &document);
        let result = evaluate_fixture(&store, &control);
        assert!(!result.complete);
        assert_eq!(result.len(), 1);
        assert!(result[0].kept.as_deref().unwrap().contains("whole batch"));
        assert!(result[0].kept.as_deref().unwrap().contains("identity"));
    }

    #[test]
    fn genuinely_released_rows_keep_the_original_present_checkout_reason() {
        let (_fixture, store, control, mut rows) = fixture();
        for row in &mut rows {
            row.selected = false;
        }
        let mut document = receipt(&rows);
        document["state"] = serde_json::json!("detached");
        document["roots"] = serde_json::json!([]);
        for link in document["links"].as_array_mut().unwrap() {
            link["selected"] = serde_json::Value::Null;
        }
        write_snapshot_registry(&control, rows.clone(), Vec::new());
        publish_snapshot_receipt(&control, &rows[0], &document);
        let result = evaluate_fixture(&store, &control);
        assert!(result.complete);
        assert_eq!(result.len(), 2);
        for entry in result.iter() {
            assert!(entry.candidate.is_none());
            assert_eq!(
                entry.kept.as_deref(),
                Some("the checkout is present, so its root is kept")
            );
        }
    }
}
