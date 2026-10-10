//! 风险/遗漏发现服务端接线：读本地文本 → 抽取 → 应用 dismiss → 报告。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::risk_scan::{
    apply_dismissals, extract_risks, render_progress_report, RiskDismissed, RiskFinding,
    RiskTextSource, KIND_RISK_DISMISSED,
};
use mc_storage::{Database, NewEvent};
use serde_json::json;

use crate::local_text;

pub fn scan(db: &Database) -> Result<Vec<RiskFinding>, AppError> {
    let sources: Vec<RiskTextSource> = local_text::collect_local_texts(db)?
        .into_iter()
        .map(|t| RiskTextSource {
            id: t.id,
            kind: t.kind,
            text: t.text,
            at: t.at,
        })
        .collect();
    let raw = extract_risks(&sources);
    let dismissed = load_dismissals(db)?;
    Ok(apply_dismissals(&raw, &dismissed))
}

pub fn report(db: &Database) -> Result<String, AppError> {
    Ok(render_progress_report(&scan(db)?))
}

pub fn dismiss(db: &Database, finding_id: &str, at: Timestamp) -> Result<(), AppError> {
    let event = NewEvent::new(
        KIND_RISK_DISMISSED,
        at,
        json!({ "finding_id": finding_id }),
    )
    .by("user");
    db.append_events(&[event])?;
    Ok(())
}

fn load_dismissals(db: &Database) -> Result<Vec<RiskDismissed>, AppError> {
    let mut out = Vec::new();
    let mut from_seq = 0i64;
    loop {
        let batch = db.read_events(from_seq, 500)?;
        if batch.is_empty() {
            break;
        }
        for envelope in batch {
            from_seq = envelope.seq;
            if envelope.kind != KIND_RISK_DISMISSED {
                continue;
            }
            if let Some(id) = envelope
                .payload
                .get("finding_id")
                .and_then(|v| v.as_str())
            {
                out.push(RiskDismissed {
                    finding_id: id.to_string(),
                    at: envelope.at,
                });
            }
        }
    }
    Ok(out)
}
