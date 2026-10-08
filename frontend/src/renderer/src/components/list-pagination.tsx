import { Button } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'

export function ListPagination({
  page,
  pages,
  onChange
}: {
  page: number
  pages: number
  onChange: (page: number) => void
}) {
  const { t } = useI18n()
  if (pages <= 1) return null
  return (
    <nav className="flex items-center justify-center gap-3 py-3" aria-label={t('pagination.label')}>
      <Button disabled={page === 0} onClick={() => onChange(page - 1)}>
        {t('pagination.previous')}
      </Button>
      <span>{t('pagination.position', { page: page + 1, pages })}</span>
      <Button disabled={page >= pages - 1} onClick={() => onChange(page + 1)}>
        {t('pagination.next')}
      </Button>
    </nav>
  )
}
