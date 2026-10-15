fn main() {
    let base = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let root = base.join("root");
    let payload = base.join("payload");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&payload).unwrap();
    use oxide_storage::*;
    let backing = BoundedBacking::verified(BoundedBackendDescriptor {
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        payload_root: payload.clone(),
        identity: "a".repeat(16),
        verified_capacity_bytes: 1024 * 1024,
    }).unwrap();
    let result = Daemon::bind(
        DaemonConfig { root: root.clone(), budget: Budget { cap_bytes: 128, free_reserve_bytes: 20 }, backing: Some(backing) },
        std::sync::Arc::new(RejectingAuthority),
        std::sync::Arc::new(Probe),
    );
    println!("bind: {result:?}");
}
struct RejectingAuthority;
impl EvidenceAuthority for RejectingAuthority {
    fn verify_admission(&self, _: &RunEnvelope) -> bool { false }
    fn verify_settlement(&self, _: &SettlementEvidence) -> bool { false }
}
struct Probe;
impl FreeSpaceProbe for Probe {
    fn available_bytes(&self, _: &std::path::Path) -> Result<Option<u64>, StorageError> { Ok(Some(1024)) }
}
