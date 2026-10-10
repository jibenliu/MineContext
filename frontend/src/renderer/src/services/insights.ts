import axiosInstance from '@renderer/services/axios-config'
import { get } from 'lodash'

export interface RiskFinding {
  id: string
  kind: string
  text: string
  source_id: string
  source_kind: string
  at: number
  score: number
}

export interface ContactEvent {
  contact_id: string
  display_name: string
  summary: string
  source_id: string
  at: number
}

export interface FollowUpHint {
  contact_id: string
  hint: string
  reason: string
  at: number
}

export interface VisitPrepPack {
  contact_id: string
  display_name: string
  prep_notes: string
  recent: Array<{ summary: string; at: number }>
  commitments: Array<{ text: string; at: number }>
  follow_ups: Array<{ hint: string; reason: string }>
}

export interface LearningTopic {
  topic_id: string
  label: string
  hit_count: number
  last_at: number
  source_ids: string[]
}

export interface StuckPattern {
  pattern_id: string
  label: string
  repeats: number
  last_at: number
  hint: string
}

export interface ReviewPlan {
  generated_at: number
  summary: string
  items: Array<{
    topic_id: string
    label: string
    due_at: number
    interval_days: number
    reason: string
  }>
}

export interface HandoffCandidate {
  id: string
  kind: string
  title: string
  body: string
  source_id: string
  at: number
  confirmed: boolean
}

export interface HandoffExport {
  manifest: {
    format: string
    schema_version: number
    exported_at_ms: number
    item_count: number
  }
  items: Array<{
    id: string
    kind: string
    title: string
    body: string
    confirmed_at: number
    confirmed_by: string
  }>
  markdown: string
}

export async function listRisks(): Promise<RiskFinding[]> {
  const res = await axiosInstance.get('/api/v1/risks')
  const findings = get(res, 'data.data.findings')
  return Array.isArray(findings) ? findings : []
}

export async function getRiskReport(): Promise<string> {
  const res = await axiosInstance.get('/api/v1/risks/report')
  return String(get(res, 'data.data.markdown') ?? '')
}

export async function dismissRisk(findingId: string): Promise<void> {
  await axiosInstance.post('/api/v1/risks/dismiss', { finding_id: findingId })
}

export async function getSalesTimeline(): Promise<ContactEvent[]> {
  const res = await axiosInstance.get('/api/v1/sales/timeline')
  const events = get(res, 'data.data.events')
  return Array.isArray(events) ? events : []
}

export async function getFollowUps(): Promise<FollowUpHint[]> {
  const res = await axiosInstance.get('/api/v1/sales/follow-ups')
  const followUps = get(res, 'data.data.follow_ups')
  return Array.isArray(followUps) ? followUps : []
}

export async function getVisitPrep(contactId: string): Promise<VisitPrepPack | null> {
  const res = await axiosInstance.get('/api/v1/sales/visit-prep', {
    params: { contact_id: contactId }
  })
  return (get(res, 'data.data.pack') as VisitPrepPack | null) ?? null
}

export async function getLearningTopics(): Promise<LearningTopic[]> {
  const res = await axiosInstance.get('/api/v1/learning/topics')
  const topics = get(res, 'data.data.topics')
  return Array.isArray(topics) ? topics : []
}

export async function getStuckPatterns(): Promise<StuckPattern[]> {
  const res = await axiosInstance.get('/api/v1/learning/stuck')
  const patterns = get(res, 'data.data.patterns')
  return Array.isArray(patterns) ? patterns : []
}

export async function getReviewPlan(): Promise<ReviewPlan> {
  const res = await axiosInstance.get('/api/v1/learning/review-plan')
  const data = get(res, 'data.data') as ReviewPlan | undefined
  return {
    generated_at: data?.generated_at ?? 0,
    summary: data?.summary ?? '',
    items: Array.isArray(data?.items) ? data!.items : []
  }
}

export async function listHandoffCandidates(): Promise<HandoffCandidate[]> {
  const res = await axiosInstance.get('/api/v1/handoff/candidates')
  const candidates = get(res, 'data.data.candidates')
  return Array.isArray(candidates) ? candidates : []
}

export async function confirmHandoff(candidateId: string): Promise<void> {
  await axiosInstance.post('/api/v1/handoff/confirm', { candidate_id: candidateId })
}

export async function exportHandoff(): Promise<HandoffExport> {
  const res = await axiosInstance.get('/api/v1/handoff/export')
  const data = get(res, 'data.data') as HandoffExport
  return {
    manifest: data?.manifest ?? {
      format: '',
      schema_version: 0,
      exported_at_ms: 0,
      item_count: 0
    },
    items: Array.isArray(data?.items) ? data.items : [],
    markdown: data?.markdown ?? ''
  }
}
