//! Large evidence on Odori: content travels to the workflow by reference, so
//! a screenshot bigger than a gRPC message still becomes evidence, and the
//! event log every activity receives stays small.

use std::{sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kyumei_app::{Command, EvidenceSubmission, InvestigationService, fixture};
use kyumei_domain::{RawBody, SubmissionChannel};
use kyumei_knowledge::KnowledgeBase;
use kyumei_odori::{DurableConfig, DurableInvestigations};

/// Deterministic incompressible bytes.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_evidence_travels_by_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let service = DurableInvestigations::start(
        kb.clone(),
        DurableConfig {
            snapshot_interval: Duration::from_secs(60),
            ..DurableConfig::new(dir.path())
        },
    )
    .await
    .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    // A 3 MiB screenshot: base64 alone exceeds the 4 MiB gRPC message limit.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(noise(3 * 1024 * 1024));
    let screenshot = service
        .command(
            &id,
            Command::SubmitEvidence(EvidenceSubmission {
                bytes_base64: Some(STANDARD.encode(&png)),
                text: None,
                filename: Some("wft-dashboard.png".into()),
                media_type: Some("image/png".into()),
                ..EvidenceSubmission::text(SubmissionChannel::Upload, "")
            }),
        )
        .await
        .expect("screenshot accepted")
        .evidence
        .expect("an evidence ID");

    // A 1 MiB log, pasted.
    let log: String = (0..20_000)
        .map(|i| {
            format!(
                "2026-09-30T13:{:02}:{:02}Z level=info msg=\"poll completed\" n={i}\n",
                35 + i / 3600 % 15,
                i % 60
            )
        })
        .collect();
    assert!(log.len() > 1024 * 1024);
    let pasted = service
        .command(
            &id,
            Command::SubmitEvidence(EvidenceSubmission::text(
                SubmissionChannel::Paste,
                log.clone(),
            )),
        )
        .await
        .expect("log accepted")
        .evidence
        .expect("an evidence ID");

    let snapshot = service.snapshot(&id).await.expect("snapshot");
    for (eid, bytes) in [(&screenshot, png.as_slice()), (&pasted, log.as_bytes())] {
        let e = snapshot.investigation.evidence(eid).expect("recorded");
        assert_eq!(e.raw.size_bytes, bytes.len() as u64);
        assert_eq!(e.raw.sha256, kyumei_evidence::sha256_hex(bytes));
        let RawBody::Stored { reference } = &e.raw.body else {
            panic!("{eid} is stored by reference");
        };
        assert_eq!(
            service.blobs().get(reference).expect("read").as_deref(),
            Some(bytes),
            "{eid} is preserved byte for byte"
        );
    }
    let log_size = serde_json::to_vec(&snapshot.investigation.events)
        .expect("json")
        .len();
    assert!(
        log_size < 256 * 1024,
        "the event log stays small ({log_size} bytes)"
    );
    service.shutdown().await.expect("shutdown");
}
