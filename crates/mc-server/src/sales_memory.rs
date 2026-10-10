//! 销售跟进记忆服务端：本地观测 → 时间线 / 承诺 / 拜访包。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::sales_memory::{
    build_contact_timeline, build_visit_prep, extract_commitments, suggest_follow_ups,
    ContactEvent, FollowUpHint, SalesCommitment, SalesObservation, VisitPrepPack,
};
use mc_storage::Database;

use crate::local_text;

fn observations(db: &Database) -> Result<Vec<SalesObservation>, AppError> {
    Ok(local_text::collect_local_texts(db)?
        .into_iter()
        .map(|t| SalesObservation {
            id: t.id,
            text: t.text,
            at: t.at,
        })
        .collect())
}

pub fn timeline(db: &Database) -> Result<Vec<ContactEvent>, AppError> {
    Ok(build_contact_timeline(&observations(db)?))
}

pub fn commitments(db: &Database) -> Result<Vec<SalesCommitment>, AppError> {
    Ok(extract_commitments(&observations(db)?))
}

pub fn follow_ups(db: &Database, now: Timestamp) -> Result<Vec<FollowUpHint>, AppError> {
    let obs = observations(db)?;
    let tl = build_contact_timeline(&obs);
    let commits = extract_commitments(&obs);
    Ok(suggest_follow_ups(&tl, &commits, now))
}

pub fn visit_prep(
    db: &Database,
    contact_id: &str,
    now: Timestamp,
) -> Result<Option<VisitPrepPack>, AppError> {
    let obs = observations(db)?;
    let tl = build_contact_timeline(&obs);
    let commits = extract_commitments(&obs);
    let hints = suggest_follow_ups(&tl, &commits, now);
    Ok(build_visit_prep(contact_id, &tl, &commits, &hints))
}
