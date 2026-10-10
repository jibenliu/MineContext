//! 交接包服务端：候选 → 用户确认事件 → 仅确认项可导出。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::handoff_pack::{
    confirmed_items, export_pack, propose_candidates, HandoffCandidate, HandoffConfirmation,
    HandoffPack, KIND_HANDOFF_CONFIRMED,
};
use mc_storage::{Database, NewEvent};
use serde_json::json;

use crate::local_text;

pub fn candidates(db: &Database) -> Result<Vec<HandoffCandidate>, AppError> {
    let sources: Vec<(String, String, Timestamp)> = local_text::collect_local_texts(db)?
        .into_iter()
        .map(|t| (t.id, t.text, t.at))
        .collect();
    Ok(propose_candidates(&sources))
}

pub fn confirm(db: &Database, candidate_id: &str, at: Timestamp) -> Result<(), AppError> {
    let event = NewEvent::new(
        KIND_HANDOFF_CONFIRMED,
        at,
        json!({ "candidate_id": candidate_id, "by": "user" }),
    )
    .by("user");
    db.append_events(&[event])?;
    Ok(())
}

pub fn load_confirmations(db: &Database) -> Result<Vec<HandoffConfirmation>, AppError> {
    let mut out = Vec::new();
    let mut from_seq = 0i64;
    loop {
        let batch = db.read_events(from_seq, 500)?;
        if batch.is_empty() {
            break;
        }
        for envelope in batch {
            from_seq = envelope.seq;
            if envelope.kind != KIND_HANDOFF_CONFIRMED {
                continue;
            }
            let Some(id) = envelope
                .payload
                .get("candidate_id")
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let by = envelope
                .payload
                .get("by")
                .and_then(|v| v.as_str())
                .unwrap_or("user");
            out.push(HandoffConfirmation {
                candidate_id: id.to_string(),
                at: envelope.at,
                by: by.to_string(),
            });
        }
    }
    Ok(out)
}

pub fn pack(db: &Database, at: Timestamp) -> Result<HandoffPack, AppError> {
    let cands = candidates(db)?;
    let confs = load_confirmations(db)?;
    let items = confirmed_items(&cands, &confs);
    Ok(export_pack(&items, at))
}
