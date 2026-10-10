// 商业方向 3–6 的可用入口：风险遗漏 / 客户跟进 / 学习教练 / 本地交接。
// 全部读本地 daemon；不出网；交接仅含用户确认条目。

import { Button, Empty, List, Message, Tabs, Tag, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import {
  confirmHandoff,
  ContactEvent,
  dismissRisk,
  exportHandoff,
  FollowUpHint,
  getFollowUps,
  getLearningTopics,
  getReviewPlan,
  getRiskReport,
  getSalesTimeline,
  getStuckPatterns,
  getVisitPrep,
  HandoffCandidate,
  HandoffExport,
  LearningTopic,
  listHandoffCandidates,
  listRisks,
  ReviewPlan,
  RiskFinding,
  StuckPattern,
  VisitPrepPack
} from '@renderer/services/insights'
import { FC, useCallback, useEffect, useState } from 'react'

const { Title, Paragraph, Text } = Typography
const TabPane = Tabs.TabPane

function kindLabel(kind: string, t: (k: string) => string): string {
  const map: Record<string, string> = {
    commitment: t('insights.risk.commitment'),
    open_question: t('insights.risk.openQuestion'),
    waiting_on: t('insights.risk.waitingOn'),
    unresolved: t('insights.risk.unresolved'),
    ops_note: t('insights.handoff.opsNote'),
    incident: t('insights.handoff.incident'),
    decision: t('insights.handoff.decision')
  }
  return map[kind] ?? kind
}

export const InsightsPage: FC = () => {
  const { t } = useI18n()
  const [risks, setRisks] = useState<RiskFinding[]>([])
  const [report, setReport] = useState('')
  const [timeline, setTimeline] = useState<ContactEvent[]>([])
  const [followUps, setFollowUps] = useState<FollowUpHint[]>([])
  const [visitPrep, setVisitPrep] = useState<VisitPrepPack | null>(null)
  const [topics, setTopics] = useState<LearningTopic[]>([])
  const [stuck, setStuck] = useState<StuckPattern[]>([])
  const [plan, setPlan] = useState<ReviewPlan | null>(null)
  const [candidates, setCandidates] = useState<HandoffCandidate[]>([])
  const [pack, setPack] = useState<HandoffExport | null>(null)
  const [loading, setLoading] = useState(true)

  const refreshRisks = useCallback(async () => {
    const [findings, md] = await Promise.all([listRisks(), getRiskReport()])
    setRisks(findings)
    setReport(md)
  }, [])

  const refreshSales = useCallback(async () => {
    const [events, hints] = await Promise.all([getSalesTimeline(), getFollowUps()])
    setTimeline(events)
    setFollowUps(hints)
    const first = events[0]?.contact_id
    if (first) {
      setVisitPrep(await getVisitPrep(first))
    } else {
      setVisitPrep(null)
    }
  }, [])

  const refreshLearning = useCallback(async () => {
    const [topicList, patterns, review] = await Promise.all([getLearningTopics(), getStuckPatterns(), getReviewPlan()])
    setTopics(topicList)
    setStuck(patterns)
    setPlan(review)
  }, [])

  const refreshHandoff = useCallback(async () => {
    const [cands, exported] = await Promise.all([listHandoffCandidates(), exportHandoff()])
    setCandidates(cands)
    setPack(exported)
  }, [])

  const refreshAll = useCallback(async () => {
    setLoading(true)
    try {
      await Promise.all([refreshRisks(), refreshSales(), refreshLearning(), refreshHandoff()])
    } catch {
      Message.error(t('insights.loadFailed'))
    } finally {
      setLoading(false)
    }
  }, [refreshRisks, refreshSales, refreshLearning, refreshHandoff, t])

  useEffect(() => {
    void refreshAll()
  }, [refreshAll])

  return (
    <div className="insights-page h-full w-full overflow-y-auto p-6" data-testid="insights-page">
      <Title heading={4} style={{ marginBottom: 8 }}>
        {t('insights.title')}
      </Title>
      <Paragraph type="secondary" style={{ marginBottom: 16 }}>
        {t('insights.subtitle')}
      </Paragraph>

      <Tabs defaultActiveTab="risks" lazyload={false}>
        <TabPane key="risks" title={t('insights.tab.risks')} data-testid="insights-tab-risks">
          <div data-testid="insights-risks-panel">
            <div className="mb-3 flex gap-2">
              <Button size="small" onClick={() => void refreshRisks()} loading={loading}>
                {t('insights.refresh')}
              </Button>
            </div>
            {report ? (
              <pre
                data-testid="insights-risk-report"
                className="mb-4 whitespace-pre-wrap rounded bg-[var(--color-fill-2)] p-3 text-sm">
                {report}
              </pre>
            ) : null}
            {risks.length === 0 ? (
              <Empty description={t('insights.risk.empty')} />
            ) : (
              <List
                dataSource={risks}
                render={(item) => (
                  <List.Item
                    key={item.id}
                    actions={[
                      <Button
                        key="dismiss"
                        size="mini"
                        type="text"
                        data-testid={`risk-dismiss-${item.id}`}
                        onClick={async () => {
                          await dismissRisk(item.id)
                          Message.success(t('insights.risk.dismissed'))
                          await refreshRisks()
                        }}>
                        {t('insights.risk.dismiss')}
                      </Button>
                    ]}>
                    <Tag size="small" color="orangered" className="mr-2">
                      {kindLabel(item.kind, t)}
                    </Tag>
                    <Text>{item.text}</Text>
                  </List.Item>
                )}
              />
            )}
          </div>
        </TabPane>

        <TabPane key="sales" title={t('insights.tab.sales')}>
          <div data-testid="insights-sales-panel">
            <Title heading={6}>{t('insights.sales.timeline')}</Title>
            {timeline.length === 0 ? (
              <Empty description={t('insights.sales.empty')} />
            ) : (
              <List
                dataSource={timeline}
                render={(item) => (
                  <List.Item key={`${item.source_id}-${item.at}`}>
                    <Text bold>{item.display_name}</Text>
                    <Text type="secondary"> — {item.summary}</Text>
                  </List.Item>
                )}
              />
            )}
            <Title heading={6} style={{ marginTop: 16 }}>
              {t('insights.sales.followUps')}
            </Title>
            <List
              dataSource={followUps}
              render={(item) => (
                <List.Item key={`${item.contact_id}-${item.hint}`}>
                  <Text>{item.hint}</Text>
                </List.Item>
              )}
            />
            {visitPrep ? (
              <pre
                data-testid="insights-visit-prep"
                className="mt-4 whitespace-pre-wrap rounded bg-[var(--color-fill-2)] p-3 text-sm">
                {visitPrep.prep_notes}
              </pre>
            ) : null}
          </div>
        </TabPane>

        <TabPane key="learning" title={t('insights.tab.learning')}>
          <div data-testid="insights-learning-panel">
            <Title heading={6}>{t('insights.learning.topics')}</Title>
            {topics.length === 0 ? (
              <Empty description={t('insights.learning.empty')} />
            ) : (
              <List
                dataSource={topics}
                render={(item) => (
                  <List.Item key={item.topic_id}>
                    <Text bold>{item.label}</Text>
                    <Text type="secondary"> ×{item.hit_count}</Text>
                  </List.Item>
                )}
              />
            )}
            <Title heading={6} style={{ marginTop: 16 }}>
              {t('insights.learning.stuck')}
            </Title>
            <List
              dataSource={stuck}
              render={(item) => (
                <List.Item key={item.pattern_id}>
                  <Text>{item.hint}</Text>
                </List.Item>
              )}
            />
            <Title heading={6} style={{ marginTop: 16 }}>
              {t('insights.learning.plan')}
            </Title>
            {plan ? (
              <>
                <Paragraph data-testid="insights-review-summary">{plan.summary}</Paragraph>
                <List
                  dataSource={plan.items}
                  render={(item) => (
                    <List.Item key={`${item.topic_id}-${item.interval_days}`}>
                      <Text>
                        {item.label} · D+{item.interval_days}
                      </Text>
                    </List.Item>
                  )}
                />
              </>
            ) : null}
          </div>
        </TabPane>

        <TabPane key="handoff" title={t('insights.tab.handoff')}>
          <div data-testid="insights-handoff-panel">
            <Paragraph type="secondary">{t('insights.handoff.hint')}</Paragraph>
            <List
              dataSource={candidates}
              render={(item) => (
                <List.Item
                  key={item.id}
                  actions={
                    item.confirmed
                      ? [<Tag key="ok">{t('insights.handoff.confirmed')}</Tag>]
                      : [
                          <Button
                            key="confirm"
                            size="mini"
                            type="primary"
                            data-testid={`handoff-confirm-${item.id}`}
                            onClick={async () => {
                              await confirmHandoff(item.id)
                              Message.success(t('insights.handoff.confirmOk'))
                              await refreshHandoff()
                            }}>
                            {t('insights.handoff.confirm')}
                          </Button>
                        ]
                  }>
                  <Tag size="small" className="mr-2">
                    {kindLabel(item.kind, t)}
                  </Tag>
                  <Text>{item.title}</Text>
                </List.Item>
              )}
            />
            <div className="mt-4">
              <Button
                type="outline"
                data-testid="handoff-export"
                onClick={async () => {
                  const exported = await exportHandoff()
                  setPack(exported)
                  Message.success(t('insights.handoff.exported').replace('{n}', String(exported.manifest.item_count)))
                }}>
                {t('insights.handoff.export')}
              </Button>
            </div>
            {pack ? (
              <pre
                data-testid="insights-handoff-markdown"
                className="mt-4 whitespace-pre-wrap rounded bg-[var(--color-fill-2)] p-3 text-sm">
                {pack.markdown}
              </pre>
            ) : null}
          </div>
        </TabPane>
      </Tabs>
    </div>
  )
}

export default InsightsPage
