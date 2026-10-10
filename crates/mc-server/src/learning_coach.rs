//! 学习轨迹 / 间隔复习服务端接线。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::learning_coach::{
    build_spaced_review_plan, detect_stuck_patterns, detect_topics, LearningObservation,
    LearningTopic, SpacedReviewPlan, StuckPattern,
};
use mc_storage::Database;

use crate::local_text;

fn observations(db: &Database) -> Result<Vec<LearningObservation>, AppError> {
    Ok(local_text::collect_local_texts(db)?
        .into_iter()
        .map(|t| LearningObservation {
            id: t.id,
            text: t.text,
            at: t.at,
        })
        .collect())
}

pub fn topics(db: &Database) -> Result<Vec<LearningTopic>, AppError> {
    Ok(detect_topics(&observations(db)?))
}

pub fn stuck(db: &Database) -> Result<Vec<StuckPattern>, AppError> {
    Ok(detect_stuck_patterns(&observations(db)?))
}

pub fn review_plan(db: &Database, now: Timestamp) -> Result<SpacedReviewPlan, AppError> {
    let obs = observations(db)?;
    let topics = detect_topics(&obs);
    let stuck = detect_stuck_patterns(&obs);
    Ok(build_spaced_review_plan(&topics, &stuck, now))
}
