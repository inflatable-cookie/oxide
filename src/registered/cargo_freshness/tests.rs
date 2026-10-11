use super::*;

const CHECK_DEP_INFO: &[u8] = include_bytes!("fixtures/check-dep-info.json");
const RERUN_FILES: &[u8] = include_bytes!("fixtures/rerun-files.json");
const RERUN_ENVIRONMENT: &[u8] = include_bytes!("fixtures/rerun-environment.json");
const PRECALCULATED: &[u8] = include_bytes!("fixtures/precalculated.json");

fn decode(bytes: &[u8]) -> Result<Fingerprint> {
    let mut budget = EvidenceReadBudget::new(
        super::super::Limits::NUCLEUS.collection_bytes,
        super::super::Limits::NUCLEUS,
    );
    decode_fingerprint(bytes, &mut budget)
}

#[test]
fn hand_annotated_pinned_variants_and_environment_presence_parse() {
    let check = decode(CHECK_DEP_INFO).unwrap();
    assert!(matches!(
        &check.local[..],
        [LocalFingerprint::CheckDepInfo(CheckDepInfo {
            dep_info,
            checksum: false
        })] if dep_info == "debug/deps/source.d"
    ));
    assert!(matches!(
        decode(RERUN_FILES).unwrap().local.as_slice(),
        [LocalFingerprint::RerunIfChanged(RerunIfChanged { output, paths })]
            if output == "debug/.fingerprint/script-hash/output"
                && paths.len() == 2
                && paths[0] == "build-input"
                && paths[1] == "data"
    ));
    assert!(matches!(
        decode(RERUN_ENVIRONMENT).unwrap().local.as_slice(),
        [
            LocalFingerprint::RerunIfEnvChanged(RerunIfEnvChanged { var: absent, val: None }),
            LocalFingerprint::RerunIfEnvChanged(RerunIfEnvChanged { var: empty, val: Some(value) })
        ] if absent == "ABSENT" && empty == "EMPTY" && value.is_empty()
    ));
    assert!(matches!(
        decode(PRECALCULATED).unwrap().local.as_slice(),
        [LocalFingerprint::Precalculated(value)] if value == "immutable package identity"
    ));
}

#[test]
fn rejects_unknown_fields_variants_duplicate_keys_and_truncated_json() {
    for value in [
        String::from_utf8(CHECK_DEP_INFO.to_vec())
            .unwrap()
            .replace("\"rustc\": 1,", "\"rustc\": 1, \"future\": 2,"),
        String::from_utf8(CHECK_DEP_INFO.to_vec()).unwrap().replace(
            "\"checksum\": false",
            "\"checksum\": false, \"future\": true",
        ),
        String::from_utf8(CHECK_DEP_INFO.to_vec())
            .unwrap()
            .replace("\"CheckDepInfo\"", "\"FutureLocalFingerprint\""),
        String::from_utf8(CHECK_DEP_INFO.to_vec())
            .unwrap()
            .replace("\"rustc\": 1,", "\"rustc\": 1, \"rustc\": 1,"),
    ] {
        assert!(decode(value.as_bytes()).is_err());
    }
    assert!(decode(&CHECK_DEP_INFO[..CHECK_DEP_INFO.len() - 2]).is_err());
}

#[test]
fn rejects_duplicate_enum_payload_and_unknown_dependency_shape() {
    let duplicate_variant = String::from_utf8(CHECK_DEP_INFO.to_vec())
        .unwrap()
        .replace(
            "{ \"CheckDepInfo\": { \"dep_info\": \"debug/deps/source.d\", \"checksum\": false } }",
            "{ \"CheckDepInfo\": { \"dep_info\": \"debug/deps/source.d\", \"checksum\": false }, \"Precalculated\": \"x\" }",
        );
    assert!(decode(duplicate_variant.as_bytes()).is_err());

    let malformed_dependency = String::from_utf8(CHECK_DEP_INFO.to_vec()).unwrap().replace(
        "[17, \"script-unit\", false, 987]",
        "[17, \"script-unit\", false]",
    );
    assert!(decode(malformed_dependency.as_bytes()).is_err());

    let extra_dependency_field = String::from_utf8(CHECK_DEP_INFO.to_vec()).unwrap().replace(
        "[17, \"script-unit\", false, 987]",
        "[17, \"script-unit\", false, 987, 12]",
    );
    assert!(decode(extra_dependency_field.as_bytes()).is_err());
}
