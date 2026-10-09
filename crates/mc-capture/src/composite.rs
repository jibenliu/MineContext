//! 多源合并（`capture.sources`）。
//!
//! 职责：把 `capture.sources` 里配置的源真正都跑起来（而不是只跑屏幕），
//! 目标合并、**并发轮询**（结果按源的顺序合并，顺序对消费方是契约）、
//! 每条观测带来源、单源失败不拖垮其它源。
//!
//! 与 `SourceRegistry` 的关系：注册表是「持有的源集合」，`CompositeSource`
//! 是把它适配成**一个** `CaptureSource`（上层只需要一个源，不必到处写循环）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mc_common::error::AppError;

use crate::source::{
    CaptureContext, CaptureSource, CaptureTarget, RawCapture, SourceCapabilities, SourceHealth,
    SourceKind,
};

/// 没有任何子源时的 id。
pub const COMPOSITE_SOURCE_ID: &str = "composite";

pub struct CompositeSource {
    /// `{子源 id}|{子源 id}`：诊断页要能一眼看出「这次跑的是哪几个源」。
    id: String,
    sources: Vec<Arc<dyn CaptureSource>>,
    /// 上一轮失败原因。`poll` 是 `&self`，因此这里用内部可变性；
    /// 锁只在轮询结束后短暂持有，不跨 `await`。
    last_errors: Mutex<BTreeMap<String, String>>,
}

impl CompositeSource {
    pub fn new(sources: Vec<Arc<dyn CaptureSource>>) -> Self {
        let id = if sources.is_empty() {
            COMPOSITE_SOURCE_ID.to_string()
        } else {
            sources.iter().map(|s| s.id()).collect::<Vec<_>>().join("|")
        };

        Self {
            id,
            sources,
            last_errors: Mutex::new(BTreeMap::new()),
        }
    }

    /// 已启用的源 id，顺序与配置一致（诊断与设置页展示用）。
    pub fn ids(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.id()).collect()
    }

    /// 上一轮 `poll` 中失败的源及原因。空 = 全部正常。
    pub fn last_errors(&self) -> BTreeMap<String, String> {
        mc_common::lock::lock_or_recover(&self.last_errors).clone()
    }

    fn update_errors(&self, errors: BTreeMap<String, String>) {
        *mc_common::lock::lock_or_recover(&self.last_errors) = errors;
    }
}

#[async_trait]
impl CaptureSource for CompositeSource {
    fn id(&self) -> &str {
        &self.id
    }

    /// 组合源本身没有「一种」类型；报告第一个子源的类型。
    /// 需要精确类型的地方请用 `RawCapture::source_kind`。
    fn kind(&self) -> SourceKind {
        self.sources
            .first()
            .map(|s| s.kind())
            .unwrap_or(SourceKind::Screen)
    }

    /// 能力取并集：任一子源能产图像，组合源就产图像。
    fn capabilities(&self) -> SourceCapabilities {
        let mut capabilities = SourceCapabilities {
            produces_image: false,
            produces_text: false,
            needs_permission_granted: false,
        };
        for source in &self.sources {
            let c = source.capabilities();
            capabilities.produces_image |= c.produces_image;
            capabilities.produces_text |= c.produces_text;
            capabilities.needs_permission_granted |= c.needs_permission_granted;
        }
        capabilities
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        // 并发枚举：屏幕源要问系统要显示器与窗口列表（几百毫秒），
        // 串行会把等待时间叠加。结果按**源的顺序**合并，消费方看到的顺序不变。
        let results =
            futures_util::future::join_all(self.sources.iter().map(|source| source.enumerate()))
                .await;

        let mut merged: Vec<CaptureTarget> = Vec::new();
        let mut first_error: Option<AppError> = None;

        for result in results {
            match result {
                Ok(targets) => {
                    for target in targets {
                        // 同一个目标被两个源枚举出来时只留一份，
                        // 否则同一帧会被采集两次（白花一次分析钱）。
                        if !merged.iter().any(|existing| existing.id == target.id) {
                            merged.push(target);
                        }
                    }
                }
                // 一个源枚举不了（缺权限）不代表其它源也不行
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if merged.is_empty() {
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        Ok(merged)
    }

    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        let mut merged = Vec::new();
        let mut errors = BTreeMap::new();

        // 并发轮询：一次 tick 的耗时应该是**最慢的那个源**，而不是所有源之和
        // （屏幕截图 + 窗口元数据 + 剪贴板串起来就是三份等待）。
        // 结果按源的顺序合并：观测的先后顺序对下游（变化检测/落库）是契约。
        let results =
            futures_util::future::join_all(self.sources.iter().map(|source| source.poll(ctx)))
                .await;

        for (source, result) in self.sources.iter().zip(results) {
            match result {
                Ok(mut captures) => merged.append(&mut captures),
                // 剪贴板权限问题不该让截图停摆：记住原因，继续下一个源
                Err(error) => {
                    errors.insert(source.id().to_string(), error.detail().to_string());
                }
            }
        }

        self.update_errors(errors);
        Ok(merged)
    }

    async fn health(&self) -> SourceHealth {
        let mut available_any = false;
        let mut unavailable: Vec<String> = Vec::new();
        let mut permission = crate::source::PermissionState::NotRequired;

        for source in &self.sources {
            let health = source.health().await;
            if health.permission == crate::source::PermissionState::Denied {
                permission = crate::source::PermissionState::Denied;
            }
            if health.available {
                available_any = true;
            } else {
                unavailable.push(match health.message {
                    Some(message) => format!("{}（{message}）", source.id()),
                    None => source.id().to_string(),
                });
            }
        }

        // 全部子源都不可用才算不可用；只要有源还能采，
        // 就必须在 message 里点名少了哪个源 —— 否则用户只会看到数据莫名缺失。
        SourceHealth {
            available: available_any || self.sources.is_empty(),
            permission,
            message: if unavailable.is_empty() {
                None
            } else {
                Some(format!("以下采集源当前不可用：{}", unavailable.join("、")))
            },
        }
    }

    async fn preview_thumbnails(
        &self,
        max_width: u32,
    ) -> std::collections::HashMap<String, String> {
        let results = futures_util::future::join_all(
            self.sources
                .iter()
                .map(|source| source.preview_thumbnails(max_width)),
        )
        .await;

        let mut merged = std::collections::HashMap::new();
        for map in results {
            for (id, url) in map {
                merged.entry(id).or_insert(url);
            }
        }
        merged
    }
}
