//! Live check of the Claude Code explainer. Ignored by default: it spends a
//! model turn. Run with
//! `cargo test -p kyumei-odori --test live_claude -- --ignored --nocapture`.

use std::{sync::Arc, time::Duration};

use kyumei_app::{ExplanationStatus, InvestigationService, fixture};
use kyumei_knowledge::KnowledgeBase;
use kyumei_odori::{DurableConfig, DurableInvestigations, Explainer};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends a Claude Code turn"]
async fn claude_explains_the_golden_investigation() {
    let explainer = match std::env::var("KYUMEI_LIVE_EXPLAINER").as_deref() {
        Ok("codex") => Explainer::Codex,
        _ => Explainer::Claude,
    };
    if let Err(e) = explainer.preflight().await {
        eprintln!("skipping: {e}");
        return;
    }
    let data = tempfile::tempdir().expect("tempdir");
    let sandbox = data.path().join("agent-sandbox");
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    std::env::set_current_dir(&sandbox).expect("cwd");

    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let service = DurableInvestigations::start(
        kb.clone(),
        DurableConfig {
            data_dir: data.path().join("odori"),
            snapshot_interval: Duration::from_secs(60),
            explainer: Some(explainer.clone()),
            simulate: false,
        },
    )
    .await
    .expect("start");
    let (id, _) = fixture::replay(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let started = std::time::Instant::now();
    loop {
        let s = service.snapshot(&id).await.expect("snapshot");
        let latest = s.investigation.assessments.last().map(|(seq, _)| *seq);
        match &s.explanation {
            ExplanationStatus::Current { seq } if Some(*seq) == latest => {
                let e = s.model_explanation.expect("explanation");
                eprintln!(
                    "ACCEPTED after {:?}\n\nSUMMARY: {}\n",
                    started.elapsed(),
                    e.summary
                );
                for f in e.findings.iter().take(3) {
                    eprintln!("  {}: {}", f.finding, f.text);
                }
                break;
            }
            ExplanationStatus::Fallback { seq, reason } if Some(*seq) == latest => {
                eprintln!(
                    "REJECTED (deterministic text kept) after {:?}: {reason}",
                    started.elapsed()
                );
                break;
            }
            _ => {}
        }
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "no explanation within 10 minutes"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    service.shutdown().await.expect("shutdown");
}
